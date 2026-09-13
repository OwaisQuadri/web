#include "adapter.h"

#include "qt-smoke/src/main.rs.h"

#include <QtCore/QCryptographicHash>
#include <QtCore/QDir>
#include <QtCore/QFile>
#include <QtCore/QFileInfo>
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QPointer>
#include <QtCore/QProcess>
#include <QtCore/QSet>
#include <QtCore/QStringList>
#include <QtCore/QTimer>
#include <QtCore/QUrl>
#include <QtCore/QVariant>
#include <QtGui/QAction>
#include <QtGui/QPixmap>
#include <QtNetwork/QHostAddress>
#include <QtWebEngineCore/QWebEngineCertificateError>
#include <QtWebEngineCore/QWebEnginePage>
#include <QtWebEngineCore/QWebEnginePermission>
#include <QtWebEngineCore/QWebEngineProfile>
#include <QtWebEngineCore/QWebEngineUrlRequestInfo>
#include <QtWebEngineCore/QWebEngineUrlRequestInterceptor>
#include <QtWebEngineWidgets/QWebEngineView>
#include <QtWidgets/QApplication>
#include <QtWidgets/QLineEdit>
#include <QtWidgets/QMainWindow>
#include <QtWidgets/QSplitter>
#include <QtWidgets/QStatusBar>
#include <QtWidgets/QTabBar>
#include <QtWidgets/QTabWidget>
#include <QtWidgets/QToolBar>
#include <QtWidgets/QWidget>

#include <algorithm>
#include <array>
#include <memory>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace qt_smoke {

std::string to_string(rust::Str value) { return {value.data(), value.size()}; }

bool is_loopback_fixture_origin(const QUrl &url) {
  const QString host = url.host();
  const QHostAddress address(host);
  const bool is_localhost_name =
      host.compare(QStringLiteral("localhost"), Qt::CaseInsensitive) == 0;
  return url.isValid() && url.scheme() == QStringLiteral("http") &&
         url.port() > 0 && (is_localhost_name || address.isLoopback()) &&
         url.userInfo().isEmpty();
}

bool is_fixture_address(const QUrl &origin, const QUrl &url) {
  return is_loopback_fixture_origin(origin) && url.isValid() &&
         url.userInfo().isEmpty() && url.scheme() == origin.scheme() &&
         url.host().compare(origin.host(), Qt::CaseInsensitive) == 0 &&
         url.port() == origin.port();
}

class FixtureRequestInterceptor final : public QWebEngineUrlRequestInterceptor {
public:
  explicit FixtureRequestInterceptor(const QUrl &origin) : origin_(origin) {}

  void interceptRequest(QWebEngineUrlRequestInfo &info) override {
    if (!is_fixture_address(origin_, info.requestUrl())) {
      info.block(true);
    }
  }

private:
  QUrl origin_;
};

class SmokePage final : public QWebEnginePage {
public:
  SmokePage(QWebEngineProfile *profile, const QUrl &fixture_origin)
      : QWebEnginePage(profile), fixture_origin_(fixture_origin) {
    connect(
        this, &QWebEnginePage::certificateError, this,
        [](QWebEngineCertificateError error) { error.rejectCertificate(); });
    connect(this, &QWebEnginePage::permissionRequested, this,
            [](const QWebEnginePermission &permission) { permission.deny(); });
  }

protected:
  auto acceptNavigationRequest(const QUrl &url, NavigationType type,
                               bool is_main_frame) -> bool override {
    static_cast<void>(type);
    static_cast<void>(is_main_frame);
    return is_fixture_address(fixture_origin_, url);
  }

private:
  QUrl fixture_origin_;
};

struct Tab final {
  std::uint64_t id;
  QString label;
  std::unique_ptr<SmokePage> page;
  QWebEngineView *view;
  QUrl last_committed_url;
};

class QtSmokeAdapterImpl final {
public:
  explicit QtSmokeAdapterImpl(rust::Box<BrowserPolicy> policy)
      : policy_(std::move(policy)), argv_{application_name_.data(), nullptr} {
    if (QApplication::instance() != nullptr) {
      throw std::runtime_error("Qt application already exists on this thread");
    }
    application_ = std::make_unique<QApplication>(argc_, argv_.data());
    build_window();
    smoke_deadline_.setSingleShot(true);
    QObject::connect(&smoke_deadline_, &QTimer::timeout, window_.get(), [this] {
      if (is_manual_smoke_) {
        manual_smoke_failed(QStringLiteral("manual smoke deadline elapsed"));
      } else {
        fail_smoke(QStringLiteral("smoke deadline elapsed"));
      }
    });
  }

  ~QtSmokeAdapterImpl() {
    smoke_deadline_.stop();
    if (recording_process_ != nullptr &&
        recording_process_->state() != QProcess::NotRunning) {
      recording_process_->kill();
      recording_process_->waitForFinished(1000);
    }
    detach_split();
    destroy_tabs();
    destroy_hidden_session(hidden_two_);
    destroy_hidden_session(hidden_one_);
    visible_interceptor_.reset();
    hidden_two_.interceptor.reset();
    hidden_one_.interceptor.reset();
    visible_profile_.reset();
  }

  int run_smoke(rust::Str fixture_base_address, rust::Str evidence_directory,
                bool is_interactive, bool is_manual_smoke) {
    const QUrl fixture_origin(
        QString::fromStdString(to_string(fixture_base_address)),
        QUrl::StrictMode);
    const QString evidence_path =
        QString::fromStdString(to_string(evidence_directory));
    if (!is_loopback_fixture_origin(fixture_origin)) {
      throw std::runtime_error("fixture base address must be a credential-free "
                               "loopback http origin");
    }
    if (evidence_path.isEmpty() || !QDir().mkpath(evidence_path)) {
      throw std::runtime_error("cannot create evidence directory");
    }

    fixture_origin_ = fixture_origin;
    evidence_directory_ = evidence_path;
    is_interactive_ = is_interactive;
    is_manual_smoke_ = is_manual_smoke;
    initialize_profiles();
    add_tab(1, QStringLiteral("Tab 1"), smoke_url(QStringLiteral("visible")));
    add_tab(2, QStringLiteral("Tab 2"), frame_url());
    window_->show();
    if (is_interactive_) {
      if (is_manual_smoke_) {
        smoke_deadline_.start(30000);
      }
      const int application_result = application_->exec();
      return is_manual_smoke_ ? exit_code_ : application_result;
    }
    is_waiting_for_initial_load_ = true;
    smoke_deadline_.start(20000);
    application_->exec();
    return exit_code_;
  }

private:
  enum class CaptureStartResult { Started, Busy, AuthorizationDenied };

  struct HiddenSession final {
    std::unique_ptr<QWebEngineProfile> profile;
    std::unique_ptr<FixtureRequestInterceptor> interceptor;
    std::unique_ptr<SmokePage> page;
    std::unique_ptr<QWebEngineView> view;
    std::unique_ptr<QWidget> host;
    std::unique_ptr<rust::Box<BrowserPolicy>> policy;
    QString expected_storage;
    QString expected_input;
  };

  void build_window() {
    window_ = std::make_unique<QMainWindow>();
    window_->setWindowTitle(QStringLiteral("Qt smoke"));
    window_->resize(1200, 800);
    toolbar_ = new QToolBar(window_.get());
    address_ = new QLineEdit(toolbar_);
    address_->setMinimumWidth(420);
    toolbar_->addWidget(address_);
    navigate_action_ =
        toolbar_->addAction(QStringLiteral("Navigate"), window_.get(),
                            [this] { navigate_selected(); });
    toolbar_->addAction(QStringLiteral("Unload"), window_.get(),
                        [this] { discard_background_tab(); });
    reload_action_ =
        toolbar_->addAction(QStringLiteral("Reload"), window_.get(),
                            [this] { reload_tab(selected_tab_id()); });
    grant_action_ = toolbar_->addAction(QStringLiteral("Grant"), window_.get(),
                                        [this] { grant_selected(); });
    revoke_action_ = toolbar_->addAction(
        QStringLiteral("Revoke"), window_.get(), [this] { revoke_selected(); });
    screenshot_action_ = toolbar_->addAction(
        QStringLiteral("Screenshot"), window_.get(), [this] {
          const CaptureStartResult result = capture_screenshot();
          if (is_native_control_probe_active_) {
            native_control_capture_result_ = result;
            is_native_control_capture_result_observed_ = true;
          }
        });
    record_action_ = toolbar_->addAction(
        QStringLiteral("Record"), window_.get(), [this] { start_recording(); });
    split_action_ =
        toolbar_->addAction(QStringLiteral("Second pane"), window_.get(),
                            [this] { toggle_split(); });
    window_->addToolBar(toolbar_);

    splitter_ = new QSplitter(window_.get());
    tabs_ = new QTabWidget(splitter_);
    QObject::connect(
        tabs_, &QTabWidget::currentChanged, window_.get(), [this](int index) {
          if (index >= 0 &&
              static_cast<std::size_t>(index) < tab_records_.size()) {
            select_tab(tab_records_[static_cast<std::size_t>(index)].id, false);
          }
        });
    splitter_->addWidget(tabs_);
    window_->setCentralWidget(splitter_);
  }

  void initialize_profiles() {
    visible_profile_ =
        std::make_unique<QWebEngineProfile>(QStringLiteral("qt-smoke-visible"));
    const QDir evidence(evidence_directory_);
    visible_profile_->setPersistentStoragePath(
        evidence.filePath(QStringLiteral("visible-profile")));
    visible_profile_->setCachePath(
        evidence.filePath(QStringLiteral("visible-cache")));
    visible_profile_->setPersistentCookiesPolicy(
        QWebEngineProfile::ForcePersistentCookies);
    visible_profile_->setPersistentPermissionsPolicy(
        QWebEngineProfile::PersistentPermissionsPolicy::StoreOnDisk);
    visible_interceptor_ =
        std::make_unique<FixtureRequestInterceptor>(fixture_origin_);
    visible_profile_->setUrlRequestInterceptor(visible_interceptor_.get());
    initialize_hidden_session(hidden_one_, QStringLiteral("alpha"),
                              QStringLiteral("hidden-alpha"));
    initialize_hidden_session(hidden_two_, QStringLiteral("beta"),
                              QStringLiteral("hidden-beta"));
  }

  void initialize_hidden_session(HiddenSession &session, const QString &storage,
                                 const QString &input) {
    session.profile = std::make_unique<QWebEngineProfile>();
    session.interceptor =
        std::make_unique<FixtureRequestInterceptor>(fixture_origin_);
    session.profile->setUrlRequestInterceptor(session.interceptor.get());
    session.page =
        std::make_unique<SmokePage>(session.profile.get(), fixture_origin_);
    session.view = std::make_unique<QWebEngineView>(session.page.get());
    session.host = std::make_unique<QWidget>();
    session.host->setWindowFlags(Qt::Tool | Qt::FramelessWindowHint |
                                 Qt::WindowDoesNotAcceptFocus);
    session.host->setAttribute(Qt::WA_ShowWithoutActivating);
    session.host->setAttribute(Qt::WA_TransparentForMouseEvents);
    session.host->setFocusPolicy(Qt::NoFocus);
    session.host->setWindowOpacity(0.0);
    session.host->resize(900, 700);
    session.view->setParent(session.host.get());
    session.view->setGeometry(session.host->rect());
    const std::uint64_t session_id =
        storage == QStringLiteral("alpha") ? 101 : 102;
    session.policy = std::make_unique<rust::Box<BrowserPolicy>>(
        new_browser_policy(session_id, 1));
    QObject::connect(session.view.get(), &QWebEngineView::loadStarted,
                     window_.get(), [policy = session.policy.get()] {
                       if (policy != nullptr) {
                         is_navigation_started(**policy, 1);
                       }
                     });
    session.expected_storage = storage;
    session.expected_input = input;
  }

  void add_tab(std::uint64_t id, const QString &label, const QUrl &url) {
    auto page =
        std::make_unique<SmokePage>(visible_profile_.get(), fixture_origin_);
    auto view = std::make_unique<QWebEngineView>(page.get());
    QObject::connect(view.get(), &QWebEngineView::loadFinished, window_.get(),
                     [this, id](bool is_ok) { on_tab_loaded(id, is_ok); });
    QObject::connect(page.get(), &QWebEnginePage::lifecycleStateChanged,
                     window_.get(),
                     [this, id](QWebEnginePage::LifecycleState state) {
                       if (state == QWebEnginePage::LifecycleState::Discarded) {
                         on_tab_discarded(id);
                       }
                     });
    view->load(url);
    QWebEngineView *tab_view = view.release();
    const int index = tabs_->addTab(tab_view, label);
    tab_records_.push_back(Tab{id, label, std::move(page), tab_view, QUrl()});
    if (index == 0) {
      select_tab(id, true);
    }
  }

  void on_tab_loaded(std::uint64_t id, bool is_ok) {
    Tab *tab = tab_for(id);
    if (is_ok && tab != nullptr) {
      tab->last_committed_url = tab->view->url();
    }
    if (!is_ok && !is_interactive_) {
      fail_smoke(QStringLiteral("fixture load failed for tab %1").arg(id));
      return;
    }
    if (is_interactive_) {
      if (is_manual_smoke_) {
        advance_manual_smoke_after_load(id, is_ok);
      }
      return;
    }
    if (id != 1) {
      return;
    }
    if (is_waiting_for_initial_load_) {
      is_waiting_for_initial_load_ = false;
      start_smoke();
      return;
    }
    if (is_waiting_for_navigation_) {
      is_waiting_for_navigation_ = false;
      is_stale_target_rejected_ = !is_control_allowed(*policy_, stale_target_);
      fresh_target_ = target_for(*policy_, 1);
      is_fresh_target_allowed_ = is_control_allowed(*policy_, fresh_target_);
      revoke_tab(*policy_, 1);
      is_revoked_target_rejected_ =
          !is_control_allowed(*policy_, fresh_target_);
      is_waiting_for_discard_ = true;
      select_tab(2, true);
      discard_tab(1);
      return;
    }
    if (is_waiting_for_reload_) {
      is_waiting_for_reload_ = false;
      select_tab(1, true);
      grant_selected();
      is_split_exercised_ = is_split_enabled();
      run_visible_input_check();
    }
  }

  void on_tab_discarded(std::uint64_t id) {
    if (id != 1 || !is_waiting_for_discard_) {
      return;
    }
    if (!is_discarded(*policy_, id)) {
      fail_smoke(QStringLiteral("discarded tab was not tracked by policy"));
      return;
    }
    is_waiting_for_discard_ = false;
    is_discard_observed_ = true;
    is_waiting_for_reload_ = true;
    reload_tab(id);
  }

  void start_smoke() {
    select_tab(1, true);
    grant_selected();
    stale_target_ = target_for(*policy_, 1);
    is_first_target_allowed_ = is_control_allowed(*policy_, stale_target_);
    is_unknown_tab_rejected_ = !is_tab_selected(*policy_, 99) &&
                               !is_control_allowed(*policy_, PageTarget{1, 2, 0});
    QUrl nonfixture_address = fixture_origin_;
    nonfixture_address.setScheme(QStringLiteral("https"));
    is_nonfixture_navigation_rejected_ =
        !is_fixture_address(fixture_origin_, nonfixture_address);
    address_->setText(
        smoke_url(QStringLiteral("visible-after-navigation")).toString());
    is_waiting_for_navigation_ = true;
    navigate_selected();
  }

  void run_visible_input_check() {
    Tab *tab = tab_for(1);
    if (tab == nullptr) {
      fail_smoke(QStringLiteral("visible tab disappeared"));
      return;
    }
    tab->page->runJavaScript(
        QStringLiteral(
            "(() => { const input = document.querySelector('#input'); const "
            "frame = document.querySelector('#frame'); const canvas = "
            "document.querySelector('#canvas'); input.value = 'visible-input'; "
            "input.dispatchEvent(new Event('input', { bubbles: true })); const "
            "context = canvas.getContext('2d'); context.fillStyle = '#1e90ff'; "
            "context.fillRect(0, 0, 2, 2); window.__qtVisibleInput = "
            "JSON.stringify({ input: input.value, frame: Boolean(frame), "
            "canvas: "
            "Boolean(canvas), storage: localStorage.getItem('smoke-store') }); "
            "})()"),
        [this](const QVariant &) { poll_visible_input(20); });
  }

  void poll_visible_input(int remaining_attempts) {
    Tab *tab = tab_for(1);
    if (tab == nullptr) {
      fail_smoke(QStringLiteral("visible tab disappeared"));
      return;
    }
    tab->page->runJavaScript(
        QStringLiteral("String(window.__qtVisibleInput || '')"),
        [this, remaining_attempts](const QVariant &value) {
          const QString result = value.toString();
          is_visible_input_verified_ =
              result.contains(QStringLiteral("visible-input")) &&
              result.contains(QStringLiteral("\"frame\":true")) &&
              result.contains(QStringLiteral("\"canvas\":true")) &&
              result.contains(QStringLiteral("visible-after-navigation"));
          if (is_visible_input_verified_) {
            load_hidden_sessions();
          } else if (remaining_attempts > 0) {
            QTimer::singleShot(50, window_.get(), [this, remaining_attempts] {
              poll_visible_input(remaining_attempts - 1);
            });
          } else {
            fail_smoke(QStringLiteral("visible input or frame check failed"));
          }
        });
  }

  void load_hidden_sessions() {
    QObject::connect(
        hidden_one_.view.get(), &QWebEngineView::loadFinished, window_.get(),
        [this](bool is_ok) {
          if (!is_ok) {
            fail_smoke(QStringLiteral("first hidden session failed to load"));
            return;
          }
          is_hidden_one_loaded_ = true;
          read_storage_when_ready();
        },
        Qt::SingleShotConnection);
    QObject::connect(
        hidden_two_.view.get(), &QWebEngineView::loadFinished, window_.get(),
        [this](bool is_ok) {
          if (!is_ok) {
            fail_smoke(QStringLiteral("second hidden session failed to load"));
            return;
          }
          is_hidden_two_loaded_ = true;
          read_storage_when_ready();
        },
        Qt::SingleShotConnection);
    hidden_one_.view->load(smoke_url(hidden_one_.expected_storage));
    hidden_two_.view->load(smoke_url(hidden_two_.expected_storage));
  }

  void read_storage_when_ready() {
    if (!is_hidden_one_loaded_ || !is_hidden_two_loaded_) {
      return;
    }
    start_storage_write(tab_for(1)->page.get(),
                        QStringLiteral("visible-after-navigation"),
                        QStringLiteral("visible-input"), 0);
    start_storage_write(hidden_one_.page.get(), hidden_one_.expected_storage,
                        hidden_one_.expected_input, 1);
    start_storage_write(hidden_two_.page.get(), hidden_two_.expected_storage,
                        hidden_two_.expected_input, 2);
  }

  void start_storage_write(SmokePage *page, const QString &storage,
                           const QString &input, int index) {
    const QString script =
        QStringLiteral(
            "(() => { const storage = '%1'; const input = "
            "document.querySelector('#input'); "
            "input.value = '%2'; input.dispatchEvent(new Event('input', { "
            "bubbles: true })); "
            "document.cookie = 'smoke-cookie-' + storage + '=' + storage + '; "
            "SameSite=Strict'; "
            "(async () => { try { const database = await new Promise((resolve, "
            "reject) => { "
            "const request = indexedDB.open('smoke-db'); request.onsuccess = "
            "() => resolve(request.result); "
            "request.onerror = () => reject(request.error); }); const "
            "transaction = database.transaction('values', 'readwrite'); "
            "transaction.objectStore('values').put(storage, storage); await "
            "new Promise((resolve, reject) => { "
            "transaction.oncomplete = resolve; transaction.onerror = () => "
            "reject(transaction.error); }); "
            "const readTransaction = database.transaction('values', "
            "'readonly'); const readRequest = "
            "readTransaction.objectStore('values').get(storage); const "
            "databaseValue = await new Promise((resolve, reject) => { "
            "readRequest.onsuccess = () => resolve(readRequest.result); "
            "readRequest.onerror = () => reject(readRequest.error); }); "
            "database.close(); const cache = await caches.open('smoke-cache'); "
            "const cacheKey = '/cache-' + storage; "
            "await cache.put(cacheKey, new Response(storage)); const response "
            "= await cache.match(cacheKey); "
            "const cacheValue = response ? await response.text() : ''; "
            "window.__qtStorageProof = JSON.stringify({ "
            "value: localStorage.getItem('smoke-store'), input: input.value, "
            "cookie: document.cookie, "
            "database: databaseValue, cache: cacheValue, fixture: "
            "window.fixtureResults }); } catch (error) { "
            "window.__qtStorageProof = JSON.stringify({ error: String(error) "
            "}); } })(); })()")
            .arg(storage, input);
    page->runJavaScript(script, [this, page, index](const QVariant &) {
      poll_storage_proof(page, index, 40);
    });
  }

  void poll_storage_proof(SmokePage *page, int index, int remaining_attempts) {
    page->runJavaScript(
        QStringLiteral("String(window.__qtStorageProof || '')"),
        [this, page, index, remaining_attempts](const QVariant &value) {
          const QString result = value.toString();
          if (result.isEmpty() && remaining_attempts > 0) {
            QTimer::singleShot(
                100, window_.get(), [this, page, index, remaining_attempts] {
                  poll_storage_proof(page, index, remaining_attempts - 1);
                });
            return;
          }
          storage_results_[static_cast<std::size_t>(index)] = result;
          ++storage_read_count_;
          if (storage_read_count_ == 3) {
            evaluate_storage();
          }
        });
  }

  void evaluate_storage() {
    const bool is_visible_valid = is_storage_result_valid(
        storage_results_[0], QStringLiteral("visible-after-navigation"),
        QStringLiteral("visible-input"));
    const bool is_first_hidden_valid =
        is_storage_result_valid(storage_results_[1], QStringLiteral("alpha"),
                                QStringLiteral("hidden-alpha"));
    const bool is_second_hidden_valid =
        is_storage_result_valid(storage_results_[2], QStringLiteral("beta"),
                                QStringLiteral("hidden-beta"));
    is_storage_isolated_ =
        is_visible_valid && is_first_hidden_valid && is_second_hidden_valid;
    if (!is_storage_isolated_) {
      fail_smoke(QStringLiteral("profile storage isolation check failed"));
      return;
    }
    start_capture_sequence();
  }

  bool is_storage_result_valid(const QString &result, const QString &storage,
                               const QString &input) const {
    return result.contains(QStringLiteral("\"value\":\"") + storage +
                           QStringLiteral("\"")) &&
           result.contains(QStringLiteral("\"input\":\"") + input +
                           QStringLiteral("\"")) &&
           result.contains(QStringLiteral("smoke-cookie-") + storage +
                           QStringLiteral("=") + storage) &&
           result.contains(QStringLiteral("\"database\":\"") + storage +
                           QStringLiteral("\"")) &&
           result.contains(QStringLiteral("\"cache\":\"") + storage +
                           QStringLiteral("\"")) &&
           result.contains(QStringLiteral("\"database\":\"ready\"")) &&
           result.contains(QStringLiteral("\"cache\":\"ready\"")) &&
           !result.contains(QStringLiteral("\"error\":"));
  }

  void navigate_selected() {
    const std::uint64_t id = selected_tab_id();
    Tab *tab = tab_for(id);
    const QUrl url = QUrl(address_->text(), QUrl::StrictMode);
    if (tab == nullptr || !is_fixture_address(fixture_origin_, url) ||
        !is_navigation_started(*policy_, id)) {
      status(QStringLiteral("Navigation rejected"), id);
      return;
    }
    tab->view->load(url);
  }

  void select_tab(std::uint64_t id, bool is_synchronize_native_tab) {
    Tab *tab = tab_for(id);
    if (tab == nullptr || !is_tab_selected(*policy_, id)) {
      status(QStringLiteral("Unknown tab"), id);
      return;
    }
    const int index = tabs_->indexOf(tab->view);
    if (is_synchronize_native_tab && index >= 0 &&
        tabs_->currentIndex() != index) {
      tabs_->setCurrentIndex(index);
    }
    address_->setText(tab->view->url().toString());
  }

  void grant_selected() {
    status(is_selected_tab_granted(*policy_) ? QStringLiteral("Granted")
                                             : QStringLiteral("Grant rejected"),
           selected_tab_id());
  }

  void revoke_selected() {
    const std::uint64_t id = selected_tab_id();
    revoke_tab(*policy_, id);
    status(QStringLiteral("Revoked"), id);
  }

  void discard_background_tab() {
    const std::uint64_t id = selected_tab_id() == 1 ? 2 : 1;
    discard_tab(id);
  }

  void discard_tab(std::uint64_t id) {
    Tab *tab = tab_for(id);
    if (tab == nullptr || selected_tab_id() == id) {
      status(QStringLiteral("Discard requires a background tab"), id);
      return;
    }
    tab->page->setLifecycleState(QWebEnginePage::LifecycleState::Discarded);
    status(QStringLiteral("Discard requested"), id);
  }

  void reload_tab(std::uint64_t id) {
    Tab *tab = tab_for(id);
    if (tab == nullptr || !tab->last_committed_url.isValid()) {
      status(QStringLiteral("Reload rejected"), id);
      return;
    }
    if (!is_reload_started(*policy_, id)) {
      status(QStringLiteral("Reload rejected"), id);
      return;
    }
    tab->page->setLifecycleState(QWebEnginePage::LifecycleState::Active);
    tab->view->load(tab->last_committed_url);
  }

  bool is_split_enabled() {
    Tab *second_tab = tab_for(2);
    if (second_tab == nullptr || splitter_->indexOf(second_tab->view) >= 0) {
      return false;
    }
    const int index = tabs_->indexOf(second_tab->view);
    if (index < 0) {
      return false;
    }
    tabs_->removeTab(index);
    second_tab->view->setParent(splitter_);
    splitter_->addWidget(second_tab->view);
    second_tab->view->show();
    splitter_->setSizes({600, 600});
    return true;
  }

  bool is_split_disabled() {
    Tab *second_tab = tab_for(2);
    if (second_tab == nullptr || splitter_->indexOf(second_tab->view) < 0) {
      return false;
    }
    second_tab->view->setParent(tabs_);
    tabs_->addTab(second_tab->view, second_tab->label);
    return true;
  }

  void toggle_split() {
    if (splitter_->indexOf(tab_for(2)->view) >= 0) {
      is_split_disabled();
    } else {
      is_split_enabled();
    }
  }

  void detach_split() {
    if (tab_records_.size() == 2 &&
        splitter_->indexOf(tab_records_[1].view) >= 0) {
      is_split_disabled();
    }
  }

  struct CaptureTarget final {
    BrowserPolicy *policy;
    QPointer<SmokePage> page;
    QPointer<QWebEngineView> view;
    PageTarget target;
    QString name;
    int profile_index;
  };

  bool is_capture_target_owned(const CaptureTarget &target) const {
    if (target.policy == nullptr || target.page.isNull() ||
        target.view.isNull() || target.view->page() != target.page.data()) {
      return false;
    }
    for (const Tab &tab : tab_records_) {
      if (target.policy == &*policy_ && target.page.data() == tab.page.get() &&
          target.view.data() == tab.view && target.target.tab_id == tab.id) {
        return true;
      }
    }
    for (const HiddenSession *session : {&hidden_one_, &hidden_two_}) {
      if (target.policy == &**session->policy &&
          target.page.data() == session->page.get() &&
          target.view.data() == session->view.get() &&
          target.target.tab_id == 1) {
        return true;
      }
    }
    return false;
  }

  bool is_capture_allowed(const CaptureTarget &target) const {
    return is_capture_target_owned(target) &&
           is_control_allowed(*target.policy, target.target);
  }

  bool is_fixture_pixels_present(const QPixmap &pixmap) const {
    const QImage image = pixmap.toImage();
    int dark_pixels = 0;
    int blue_pixels = 0;
    for (int y = 0; y < image.height(); y += 4) {
      for (int x = 0; x < image.width(); x += 4) {
        const QColor color = image.pixelColor(x, y);
        if (color.red() < 80 && color.green() < 80 && color.blue() < 80) {
          ++dark_pixels;
        }
        if (color.blue() > 120 && color.blue() > color.red() + 30 &&
            color.blue() > color.green() + 20) {
          ++blue_pixels;
        }
      }
    }
    return dark_pixels > 20 && blue_pixels > 20;
  }

  bool is_split_pixels_nonblank(const QPixmap &pixmap, std::uint64_t id) const {
    if (id == 1) {
      return is_fixture_pixels_present(pixmap);
    }
    const QImage image = pixmap.toImage();
    int dark_pixels = 0;
    for (int y = 0; y < image.height(); y += 2) {
      for (int x = 0; x < image.width(); x += 2) {
        const QColor color = image.pixelColor(x, y);
        if (color.red() < 80 && color.green() < 80 && color.blue() < 80) {
          ++dark_pixels;
        }
      }
    }
    return dark_pixels > 2;
  }

  QString capture_path(const CaptureTarget &target, const QString &kind,
                       const QString &file_name) const {
    const QDir profile_directory(
        QDir(evidence_directory_)
            .filePath(target.name + QStringLiteral("-") + kind));
    QDir().mkpath(profile_directory.path());
    return profile_directory.filePath(file_name);
  }

  void capture_native_controls() {
    const QDir evidence(evidence_directory_);
    const QString toolbar_path =
        evidence.filePath(QStringLiteral("native-toolbar.png"));
    const QString tabs_path =
        evidence.filePath(QStringLiteral("native-tabs.png"));
    const bool is_toolbar_written =
        toolbar_->grab().save(toolbar_path, "PNG") &&
        QFileInfo(toolbar_path).size() > 0;
    const bool is_tabs_written =
        tabs_->tabBar()->grab().save(tabs_path, "PNG") &&
        QFileInfo(tabs_path).size() > 0;
    is_native_controls_written_ = is_toolbar_written && is_tabs_written;
  }

  bool is_split_geometry_valid() const {
    const Tab *first_tab = tab_for(1);
    const Tab *second_tab = tab_for(2);
    return first_tab != nullptr && second_tab != nullptr &&
           tabs_->parentWidget() == splitter_ &&
           tabs_->indexOf(first_tab->view) >= 0 &&
           splitter_->indexOf(second_tab->view) >= 0 &&
           first_tab->view->geometry().isValid() &&
           second_tab->view->geometry().isValid() &&
           first_tab->view->geometry().width() > 0 &&
           second_tab->view->geometry().width() > 0;
  }

  bool is_hidden_host_configuration_valid() const {
    for (const HiddenSession *session : {&hidden_one_, &hidden_two_}) {
      if (session->host == nullptr || session->view == nullptr ||
          session->host->parentWidget() != nullptr ||
          session->view->parentWidget() != session->host.get() ||
          session->view->parentWidget() == splitter_ ||
          session->host->width() != 900 || session->host->height() != 700 ||
          session->view->width() != 900 || session->view->height() != 700 ||
          session->host->windowOpacity() != 0.0 ||
          session->host->focusPolicy() != Qt::NoFocus ||
          !session->host->testAttribute(Qt::WA_ShowWithoutActivating) ||
          !session->host->testAttribute(Qt::WA_TransparentForMouseEvents) ||
          !session->host->windowFlags().testFlag(
              Qt::WindowDoesNotAcceptFocus)) {
        return false;
      }
    }
    return true;
  }

  QJsonArray hidden_hosts() const {
    QJsonArray hosts;
    for (const HiddenSession *session : {&hidden_one_, &hidden_two_}) {
      QJsonObject host;
      host.insert(QStringLiteral("is_visible"), session->host->isVisible());
      host.insert(QStringLiteral("opacity"), session->host->windowOpacity());
      host.insert(QStringLiteral("no_focus"),
                  session->host->focusPolicy() == Qt::NoFocus);
      host.insert(
          QStringLiteral("does_not_accept_focus"),
          session->host->windowFlags().testFlag(Qt::WindowDoesNotAcceptFocus));
      host.insert(QStringLiteral("show_without_activating"),
                  session->host->testAttribute(Qt::WA_ShowWithoutActivating));
      host.insert(
          QStringLiteral("transparent_for_mouse"),
          session->host->testAttribute(Qt::WA_TransparentForMouseEvents));
      host.insert(QStringLiteral("separate_from_visible_splitter"),
                  session->view->parentWidget() != splitter_);
      host.insert(QStringLiteral("host_has_no_parent"),
                  session->host->parentWidget() == nullptr);
      host.insert(QStringLiteral("view_owned_by_host"),
                  session->view->parentWidget() == session->host.get());
      host.insert(QStringLiteral("width"), session->host->width());
      host.insert(QStringLiteral("height"), session->host->height());
      host.insert(QStringLiteral("engine_width"), session->view->width());
      host.insert(QStringLiteral("engine_height"), session->view->height());
      hosts.append(host);
    }
    return hosts;
  }

  QJsonObject split_geometry() const {
    const Tab *first_tab = tab_for(1);
    const Tab *second_tab = tab_for(2);
    QJsonObject geometry;
    geometry.insert(QStringLiteral("is_split"), is_split_geometry_valid());
    if (first_tab != nullptr) {
      geometry.insert(QStringLiteral("first_x"), first_tab->view->x());
      geometry.insert(QStringLiteral("first_y"), first_tab->view->y());
      geometry.insert(QStringLiteral("first_width"), first_tab->view->width());
      geometry.insert(QStringLiteral("first_height"),
                      first_tab->view->height());
    }
    if (second_tab != nullptr) {
      geometry.insert(QStringLiteral("second_x"), second_tab->view->x());
      geometry.insert(QStringLiteral("second_y"), second_tab->view->y());
      geometry.insert(QStringLiteral("second_width"),
                      second_tab->view->width());
      geometry.insert(QStringLiteral("second_height"),
                      second_tab->view->height());
    }
    return geometry;
  }

  CaptureTarget visible_capture_target() {
    return visible_capture_target_for(1);
  }

  CaptureTarget visible_capture_target_for(std::uint64_t id) {
    Tab *tab = tab_for(id);
    return CaptureTarget{&*policy_,
                         tab == nullptr ? nullptr : tab->page.get(),
                         tab == nullptr ? nullptr : tab->view,
                         target_for(*policy_, id),
                         QStringLiteral("visible"),
                         0};
  }

  CaptureTarget hidden_capture_target(HiddenSession &session,
                                      const QString &name, int profile_index) {
    return CaptureTarget{&**session.policy,
                         session.page.get(),
                         session.view.get(),
                         target_for(**session.policy, 1),
                         name,
                         profile_index};
  }

  bool is_screenshot_rejected(CaptureTarget target) {
    const int capture_sequence = capture_sequence_;
    return is_screenshot_operation_rejected(target, false) ==
               CaptureStartResult::AuthorizationDenied &&
           capture_sequence_ == capture_sequence;
  }

  bool is_screenshot_rejected_without_output(CaptureTarget target) {
    const auto is_screenshot_output = is_screenshot_written_;
    const auto is_recording_output = is_recording_written_;
    return is_screenshot_rejected(target) &&
           is_screenshot_output == is_screenshot_written_ &&
           is_recording_output == is_recording_written_;
  }

  void start_capture_sequence() {
    capture_native_controls();
    split_geometry_ = split_geometry();
    is_split_geometry_verified_ =
        split_geometry_.value(QStringLiteral("is_split")).toBool();
    is_hidden_host_configuration_verified_ =
        is_hidden_host_configuration_valid();
    if (!is_split_geometry_verified_ ||
        !is_hidden_host_configuration_verified_) {
      fail_smoke(QStringLiteral("split or hidden-host geometry is invalid"));
      return;
    }
    capture_split_engine_snapshot(1, 20);
  }

  void record_split_target_evidence(std::uint64_t id, const QString &phase,
                                    bool is_allowed) {
    QJsonObject evidence;
    evidence.insert(QStringLiteral("tab_id"), static_cast<qint64>(id));
    evidence.insert(QStringLiteral("phase"), phase);
    evidence.insert(QStringLiteral("is_authorized"), is_allowed);
    split_target_evidence_.append(evidence);
  }

  bool record_split_document_evidence(std::uint64_t id, const QVariant &value) {
    const QJsonObject document =
        QJsonDocument::fromJson(value.toString().toUtf8()).object();
    const QString expected_path =
        id == 1 ? QStringLiteral("/smoke.html") : QStringLiteral("/frame.html");
    const QString expected_text =
        id == 1 ? QStringLiteral("Engine smoke")
                : QStringLiteral("embedded fixture ready");
    const bool is_document_valid =
        document.value(QStringLiteral("path")).toString() == expected_path &&
        document.value(QStringLiteral("content"))
            .toString()
            .contains(expected_text);
    QJsonObject evidence;
    evidence.insert(QStringLiteral("tab_id"), static_cast<qint64>(id));
    evidence.insert(QStringLiteral("expected_path"), expected_path);
    evidence.insert(QStringLiteral("expected_text"), expected_text);
    evidence.insert(QStringLiteral("path"),
                    document.value(QStringLiteral("path")).toString());
    evidence.insert(QStringLiteral("content_matches_fixture"),
                    is_document_valid);
    split_document_evidence_.append(evidence);
    is_split_document_fixture_verified_[static_cast<std::size_t>(id - 1)] =
        is_document_valid;
    return is_document_valid;
  }

  void capture_split_engine_snapshot(std::uint64_t id, int remaining_attempts) {
    const QString location =
        id == 1 ? QStringLiteral("left") : QStringLiteral("right");
    select_tab(id, false);
    if (!is_selected_tab_granted(*policy_)) {
      fail_smoke(QStringLiteral("split %1 grant was rejected").arg(location));
      return;
    }
    CaptureTarget target = visible_capture_target_for(id);
    capture_split_engine_snapshot_attempt(target, location, remaining_attempts);
  }

  void capture_split_engine_snapshot_attempt(CaptureTarget target,
                                             const QString &location,
                                             int remaining_attempts) {
    const std::uint64_t id = target.target.tab_id;
    const bool is_allowed_before_document = is_capture_allowed(target);
    record_split_target_evidence(id, QStringLiteral("before_document"),
                                 is_allowed_before_document);
    if (!is_allowed_before_document) {
      fail_smoke(QStringLiteral("split %1 target authorization was not granted")
                     .arg(location));
      return;
    }
    target.page->runJavaScript(
        QStringLiteral("JSON.stringify({ path: location.pathname, content: "
                       "document.body.textContent || '' })"),
        [this, target, location, remaining_attempts](const QVariant &value) {
          const std::uint64_t id = target.target.tab_id;
          const bool is_allowed_before_acquisition = is_capture_allowed(target);
          record_split_target_evidence(id, QStringLiteral("before_acquisition"),
                                       is_allowed_before_acquisition);
          if (!is_allowed_before_acquisition ||
              !record_split_document_evidence(id, value)) {
            fail_smoke(QStringLiteral("split %1 document or target changed")
                           .arg(location));
            return;
          }
          const QPixmap image = target.view->grab();
          if (!is_split_pixels_nonblank(image, id)) {
            if (remaining_attempts > 0) {
              QTimer::singleShot(100, window_.get(),
                                 [this, target, location, remaining_attempts] {
                                   capture_split_engine_snapshot_attempt(
                                       target, location,
                                       remaining_attempts - 1);
                                 });
            } else {
              fail_smoke(QStringLiteral("split %1 nonblank pixels were not "
                                        "captured")
                             .arg(location));
            }
            return;
          }
          const Tab *tab = tab_for(id);
          const bool is_allowed_before_write = is_capture_allowed(target);
          record_split_target_evidence(id, QStringLiteral("before_write"),
                                       is_allowed_before_write);
          if (tab == nullptr || !is_allowed_before_write) {
            fail_smoke(QStringLiteral("split %1 target authorization was lost")
                           .arg(location));
            return;
          }
          const QString path = capture_path(
              target, QStringLiteral("split-snapshots"),
              QStringLiteral("split-%1-tab-%2.png").arg(location).arg(id));
          if (!image.save(path, "PNG") || QFileInfo(path).size() <= 0) {
            fail_smoke(
                QStringLiteral("split %1 snapshot write failed").arg(location));
            return;
          }
          QJsonObject snapshot;
          snapshot.insert(QStringLiteral("tab_id"), static_cast<qint64>(id));
          snapshot.insert(QStringLiteral("location"), location);
          snapshot.insert(QStringLiteral("pixels"), QStringLiteral("nonblank"));
          snapshot.insert(QStringLiteral("x"), tab->view->x());
          snapshot.insert(QStringLiteral("y"), tab->view->y());
          snapshot.insert(QStringLiteral("width"), tab->view->width());
          snapshot.insert(QStringLiteral("height"), tab->view->height());
          snapshot.insert(QStringLiteral("path"), path);
          split_engine_snapshots_.append(snapshot);
          is_split_engine_snapshot_written_[static_cast<std::size_t>(id - 1)] =
              true;
          if (id == 1) {
            capture_split_engine_snapshot(2, 20);
            return;
          }
          select_tab(1, true);
          if (!is_selected_tab_granted(*policy_)) {
            fail_smoke(
                QStringLiteral("visible capture grant could not be restored"));
            return;
          }
          CaptureTarget visible_target = visible_capture_target();
          CaptureTarget wrong_target = visible_target;
          wrong_target.target.tab_id = 2;
          is_capture_wrong_target_rejected_ =
              is_screenshot_rejected(wrong_target);
          CaptureTarget wrong_page_target = visible_target;
          const Tab *second_tab = tab_for(2);
          if (second_tab == nullptr) {
            fail_smoke(QStringLiteral("second visible tab disappeared"));
            return;
          }
          wrong_page_target.page = second_tab->page.get();
          wrong_page_target.view = second_tab->view;
          is_capture_wrong_page_rejected_ =
              is_screenshot_rejected_without_output(wrong_page_target);
          is_native_control_selected_tab_rejected_ =
              is_native_control_selected_tab_rejected();
          revoke_tab(*visible_target.policy, 1);
          is_capture_revocation_rejected_ =
              is_screenshot_rejected(visible_target);
          if (!is_selected_tab_granted(*visible_target.policy)) {
            fail_smoke(
                QStringLiteral("visible capture grant could not be restored"));
            return;
          }
          capture_profile(0);
        });
  }

  bool is_native_control_selected_tab_rejected() {
    const Tab *second_tab = tab_for(2);
    const bool is_split_enabled =
        second_tab != nullptr && splitter_->indexOf(second_tab->view) >= 0;
    if (second_tab == nullptr || grant_action_ == nullptr ||
        revoke_action_ == nullptr || screenshot_action_ == nullptr ||
        split_action_ == nullptr) {
      return false;
    }
    if (is_split_enabled) {
      split_action_->trigger();
    }
    select_tab(1, true);
    grant_action_->trigger();
    const bool is_first_tab_granted =
        selected_tab_id() == 1 && is_tab_granted(*policy_, 1);
    revoke_action_->trigger();
    const bool is_first_tab_revoked =
        selected_tab_id() == 1 && !is_tab_granted(*policy_, 1);
    grant_action_->trigger();
    const bool is_first_tab_regranted =
        selected_tab_id() == 1 && is_tab_granted(*policy_, 1);
    select_tab(2, true);
    grant_action_->trigger();
    const bool is_second_tab_granted =
        selected_tab_id() == 2 && is_tab_granted(*policy_, 2);
    revoke_action_->trigger();
    const bool is_second_tab_revoked =
        selected_tab_id() == 2 && !is_tab_granted(*policy_, 2);
    select_tab(1, true);
    grant_action_->trigger();
    select_tab(2, true);
    const int capture_sequence = capture_sequence_;
    const auto is_screenshot_output = is_screenshot_written_;
    const auto is_recording_output = is_recording_written_;
    is_native_control_capture_result_observed_ = false;
    is_native_control_capture_target_observed_ = false;
    is_native_control_probe_active_ = true;
    screenshot_action_->trigger();
    is_native_control_probe_active_ = false;
    const bool is_rejected_by_action =
        is_native_control_capture_result_observed_ &&
        native_control_capture_result_ ==
            CaptureStartResult::AuthorizationDenied;
    const bool is_second_tab_capture_requested =
        is_native_control_capture_target_observed_ &&
        native_control_capture_target_id_ == 2;
    const bool is_capture_output_unchanged =
        capture_sequence_ == capture_sequence &&
        is_screenshot_output == is_screenshot_written_ &&
        is_recording_output == is_recording_written_;
    select_tab(1, true);
    grant_action_->trigger();
    if (is_split_enabled) {
      split_action_->trigger();
    }
    return is_first_tab_granted && is_first_tab_revoked &&
           is_first_tab_regranted && is_second_tab_granted &&
           is_second_tab_revoked && selected_tab_id() == 1 &&
           is_second_tab_capture_requested && is_rejected_by_action &&
           is_capture_output_unchanged &&
           (!is_split_enabled || splitter_->indexOf(second_tab->view) >= 0);
  }

  void capture_profile(int profile_index) {
    if (is_manual_smoke_ && profile_index > 0) {
      ++manual_hidden_capture_entries_;
    }
    CaptureTarget target =
        profile_index == 0
            ? visible_capture_target()
            : (profile_index == 1
                   ? hidden_capture_target(hidden_one_,
                                           QStringLiteral("hidden-alpha"), 1)
                   : hidden_capture_target(hidden_two_,
                                           QStringLiteral("hidden-beta"), 2));
    if (profile_index > 0) {
      HiddenSession &session = profile_index == 1 ? hidden_one_ : hidden_two_;
      session.view->setGeometry(session.host->rect());
      session.host->show();
      session.view->show();
    }
    if (profile_index > 0) {
      is_hidden_capture_grant_denied_[static_cast<std::size_t>(
          profile_index - 1)] = is_screenshot_rejected(target);
    }
    if (!is_tab_selected(*target.policy, 1) ||
        !is_selected_tab_granted(*target.policy) ||
        !is_capture_allowed(target)) {
      fail_smoke(
          QStringLiteral("capture target authorization was not granted"));
      return;
    }
    static_cast<void>(is_screenshot_operation_rejected(target, true));
  }

  CaptureStartResult
  is_screenshot_operation_rejected(CaptureTarget target,
                                   bool is_followed_by_recording) {
    if (is_native_control_probe_active_) {
      native_control_capture_target_id_ = target.target.tab_id;
      is_native_control_capture_target_observed_ = true;
    }
    if (is_capture_job_active_) {
      status(QStringLiteral("Capture busy"), target.target.tab_id);
      return CaptureStartResult::Busy;
    }
    if (!is_capture_allowed(target)) {
      status(QStringLiteral("Screenshot rejected"), target.target.tab_id);
      return CaptureStartResult::AuthorizationDenied;
    }
    is_capture_job_active_ = true;
    capture_screenshot_attempt(target, 20, is_followed_by_recording);
    return CaptureStartResult::Started;
  }

  CaptureStartResult capture_screenshot() {
    const std::uint64_t id = selected_tab_id();
    return is_screenshot_operation_rejected(visible_capture_target_for(id),
                                            false);
  }

  void capture_failed(const QString &reason, const CaptureTarget &target) {
    is_capture_job_active_ = false;
    if (is_interactive_) {
      status(reason, target.target.tab_id);
      return;
    }
    fail_smoke(reason);
  }

  void capture_screenshot_attempt(CaptureTarget target, int remaining_attempts,
                                  bool is_followed_by_recording) {
    if (!is_capture_allowed(target)) {
      capture_failed(QStringLiteral("Screenshot cancelled"), target);
      return;
    }
    const QPixmap image = target.view->grab();
    if (is_fixture_pixels_present(image)) {
      const QString screenshot_path =
          capture_path(target, QStringLiteral("screenshots"),
                       QStringLiteral("screenshot-%1.png")
                           .arg(++capture_sequence_, 3, 10, QLatin1Char('0')));
      if (!is_capture_allowed(target) || !image.save(screenshot_path, "PNG") ||
          QFileInfo(screenshot_path).size() <= 0) {
        capture_failed(QStringLiteral("Screenshot failed"), target);
        return;
      }
      is_screenshot_written_[static_cast<std::size_t>(target.profile_index)] =
          true;
      if (is_followed_by_recording) {
        start_recording(target);
      } else {
        is_capture_job_active_ = false;
        if (is_manual_smoke_) {
          advance_manual_smoke_after_screenshot(target, screenshot_path, image);
        }
      }
      return;
    }
    if (remaining_attempts > 0) {
      QTimer::singleShot(
          100, window_.get(),
          [this, target, remaining_attempts, is_followed_by_recording] {
            capture_screenshot_attempt(target, remaining_attempts - 1,
                                       is_followed_by_recording);
          });
    } else {
      capture_failed(QStringLiteral("Screenshot failed: engine-buffer fixture "
                                    "pixels were not captured"),
                     target);
    }
  }

  void start_recording() {
    if (is_capture_job_active_) {
      status(QStringLiteral("Capture already active"), selected_tab_id());
      return;
    }
    const std::uint64_t id = selected_tab_id();
    CaptureTarget target = visible_capture_target_for(id);
    if (!is_capture_allowed(target)) {
      status(QStringLiteral("Recording rejected"), id);
      return;
    }
    is_capture_job_active_ = true;
    start_recording(target);
  }

  void start_recording(CaptureTarget target) {
    if (!is_capture_allowed(target)) {
      is_capture_job_active_ = false;
      status(QStringLiteral("Recording rejected"), target.target.tab_id);
      return;
    }
    recording_directory_ =
        capture_path(target, QStringLiteral("recording-frames"),
                     QStringLiteral("recording-%1")
                         .arg(++capture_sequence_, 3, 10, QLatin1Char('0')));
    if (!QDir().mkpath(recording_directory_)) {
      capture_failed(QStringLiteral("Recording failed"), target);
      return;
    }
    recording_frame_hashes_.clear();
    recording_frames_remaining_ = 5;
    if (is_revocation_probe_active_) {
      revocation_probe_target_ = target.target;
      revocation_probe_recording_directory_ = recording_directory_;
      revocation_probe_recording_path_ =
          capture_path(target, QStringLiteral("recordings"),
                       QStringLiteral("recording-%1.mp4")
                           .arg(capture_sequence_, 3, 10, QLatin1Char('0')));
    }
    record_next_frame(target);
  }

  QJsonArray recording_frame_names(const QString &directory) const {
    QJsonArray names;
    const QStringList paths =
        QDir(directory).entryList({QStringLiteral("frame-*.png")}, QDir::Files);
    for (const QString &path : paths) {
      names.append(path);
    }
    return names;
  }

  QJsonArray recording_frame_names() const {
    return recording_frame_names(recording_directory_);
  }

  void cancel_revocation_probe(CaptureTarget target) {
    revocation_probe_frames_after_ = recording_frame_names().size();
    revocation_probe_frames_after_names_ = recording_frame_names();
    is_revocation_probe_cancelled_ = true;
    is_revocation_probe_active_ = false;
    is_revocation_probe_request_pending_ = false;
    is_capture_job_active_ = false;
    finish_hidden_capture_checks(target);
  }

  void cancel_interactive_recording(const CaptureTarget &target) {
    is_capture_job_active_ = false;
    status(QStringLiteral("Recording cancelled"), target.target.tab_id);
    if (is_manual_smoke_) {
      QTimer::singleShot(0, window_.get(), [this, target] {
        if (manual_smoke_step_ ==
            ManualSmokeStep::WaitingForEncoderRevocation) {
          finish_manual_encoder_revocation(target);
        } else {
          finish_manual_revocation(target);
        }
      });
    }
  }

  void record_next_frame(CaptureTarget target) {
    if (!is_capture_allowed(target)) {
      if (is_revocation_probe_active_ && is_revocation_probe_request_pending_) {
        cancel_revocation_probe(target);
      } else if (is_interactive_) {
        cancel_interactive_recording(target);
      } else {
        fail_smoke(QStringLiteral("recording target authorization was lost"));
      }
      return;
    }
    const int frame_number = 6 - recording_frames_remaining_;
    target.page->runJavaScript(
        QStringLiteral(
            "String(document.querySelector('#frame')?.textContent || '')"),
        [this, target, frame_number](const QVariant &value) {
          const QString frame_value = value.toString();
          if (frame_value.isEmpty()) {
            if (is_interactive_) {
              cancel_interactive_recording(target);
            } else {
              fail_smoke(QStringLiteral("recording frame value was lost"));
            }
            return;
          }
          if (!is_capture_allowed(target)) {
            if (is_revocation_probe_active_ &&
                is_revocation_probe_request_pending_) {
              cancel_revocation_probe(target);
            } else if (is_interactive_) {
              cancel_interactive_recording(target);
            } else {
              fail_smoke(QStringLiteral("recording frame target was lost"));
            }
            return;
          }
          const QPixmap image = target.view->grab();
          const QString frame_path =
              QDir(recording_directory_)
                  .filePath(QStringLiteral("frame-%1.png")
                                .arg(frame_number, 3, 10, QLatin1Char('0')));
          if (!is_capture_allowed(target) ||
              !is_fixture_pixels_present(image) ||
              !image.save(frame_path, "PNG")) {
            if (is_interactive_) {
              cancel_interactive_recording(target);
            } else {
              fail_smoke(QStringLiteral("recording frame write failed"));
            }
            return;
          }
          QFile frame_file(frame_path);
          if (!frame_file.open(QIODevice::ReadOnly)) {
            if (is_interactive_) {
              cancel_interactive_recording(target);
            } else {
              fail_smoke(QStringLiteral("recording frame cannot be read"));
            }
            return;
          }
          recording_frame_hashes_.insert(QCryptographicHash::hash(
              frame_file.readAll(), QCryptographicHash::Sha256));
          if (target.profile_index == 0 && frame_number == 1) {
            const int capture_sequence = capture_sequence_;
            const QString recording_directory = recording_directory_;
            const CaptureStartResult result = capture_screenshot();
            is_capture_reentry_rejected_ =
                result == CaptureStartResult::Busy &&
                capture_sequence_ == capture_sequence &&
                recording_directory_ == recording_directory;
          }
          --recording_frames_remaining_;
          if (is_manual_smoke_ && is_manual_revocation_requested_ &&
              frame_number == 1) {
            manual_revocation_recording_directory_ = recording_directory_;
            manual_revocation_frames_before_ =
                recording_frame_names(manual_revocation_recording_directory_);
            manual_revocation_recording_path_ = capture_path(
                target, QStringLiteral("recordings"),
                QStringLiteral("recording-%1.mp4")
                    .arg(capture_sequence_, 3, 10, QLatin1Char('0')));
            manual_revocation_pending_recording_path_ =
                manual_revocation_recording_path_.left(
                    manual_revocation_recording_path_.size() - 4) +
                QStringLiteral(".pending.mp4");
            is_manual_revocation_requested_ = false;
            revoke_action_->trigger();
            QTimer::singleShot(0, window_.get(),
                               [this, target] { record_next_frame(target); });
            return;
          }
          if (is_revocation_probe_active_ && frame_number == 1) {
            revocation_probe_frames_before_ = recording_frame_names().size();
            revocation_probe_frames_before_names_ = recording_frame_names();
            is_revocation_probe_request_pending_ = true;
            record_next_frame(target);
            revoke_tab(*target.policy, target.target.tab_id);
            return;
          }
          if (recording_frames_remaining_ == 0) {
            encode_recording(target);
            return;
          }
          QTimer::singleShot(120, window_.get(),
                             [this, target] { record_next_frame(target); });
        });
  }

  void encode_recording(CaptureTarget target) {
    if (recording_frame_hashes_.size() < 2 || !is_capture_allowed(target)) {
      if (is_interactive_) {
        cancel_interactive_recording(target);
      } else {
        fail_smoke(QStringLiteral("recorded frames did not progress"));
      }
      return;
    }
    const QString recording_path =
        capture_path(target, QStringLiteral("recordings"),
                     QStringLiteral("recording-%1.mp4")
                         .arg(capture_sequence_, 3, 10, QLatin1Char('0')));
    const QString pending_recording_path =
        recording_path.left(recording_path.size() - 4) +
        QStringLiteral(".pending.mp4");
    recording_process_ = std::make_unique<QProcess>();
    QObject::connect(
        recording_process_.get(), &QProcess::errorOccurred, window_.get(),
        [this](QProcess::ProcessError error) {
          if (error == QProcess::FailedToStart) {
            if (is_manual_smoke_) {
              is_capture_job_active_ = false;
              manual_smoke_failed(
                  QStringLiteral("manual recording encoder is unavailable"));
            } else if (is_interactive_) {
              is_capture_job_active_ = false;
              status(QStringLiteral("Recording failed"), selected_tab_id());
            } else {
              block_smoke(QStringLiteral("ffmpeg is unavailable"));
            }
          }
        });
    QObject::connect(
        recording_process_.get(), &QProcess::started, window_.get(),
        [this, target, recording_path, pending_recording_path] {
          if (is_manual_smoke_ &&
              manual_smoke_step_ ==
                  ManualSmokeStep::WaitingForEncoderRevocation &&
              target.target.tab_id == 2) {
            manual_encoder_revocation_recording_directory_ =
                recording_directory_;
            manual_encoder_revocation_frames_before_ = recording_frame_names(
                manual_encoder_revocation_recording_directory_);
            manual_encoder_revocation_recording_path_ = recording_path;
            manual_encoder_revocation_pending_recording_path_ =
                pending_recording_path;
            is_manual_encoder_started_ = true;
            revoke_action_->trigger();
          }
        });
    QObject::connect(
        recording_process_.get(), &QProcess::finished, window_.get(),
        [this, target, recording_path, pending_recording_path](
            int exit_code, QProcess::ExitStatus exit_status) {
          if (!is_capture_allowed(target) ||
              exit_status != QProcess::NormalExit || exit_code != 0 ||
              QFileInfo(pending_recording_path).size() <= 0 ||
              !QFile::rename(pending_recording_path, recording_path)) {
            if (is_interactive_) {
              cancel_interactive_recording(target);
            } else {
              fail_smoke(QStringLiteral("recording encode failed"));
            }
            return;
          }
          is_recording_written_[static_cast<std::size_t>(
              target.profile_index)] = true;
          is_capture_job_active_ = false;
          if (is_interactive_) {
            status(QStringLiteral("Recording complete"), target.target.tab_id);
            if (is_manual_smoke_) {
              QTimer::singleShot(0, window_.get(),
                                 [this, target, recording_path] {
                                   advance_manual_smoke_after_recording(
                                       target, recording_path);
                                 });
            }
            return;
          }
          if (target.profile_index < 2) {
            capture_profile(target.profile_index + 1);
            return;
          }
          start_revocation_probe(target);
        });
    recording_process_->start(
        QStringLiteral("ffmpeg"),
        {QStringLiteral("-y"), QStringLiteral("-framerate"),
         QStringLiteral("5"), QStringLiteral("-i"),
         QDir(recording_directory_).filePath(QStringLiteral("frame-%03d.png")),
         QStringLiteral("-pix_fmt"), QStringLiteral("yuv420p"),
         pending_recording_path});
  }

  void start_revocation_probe(CaptureTarget target) {
    if (!is_selected_tab_granted(*target.policy)) {
      fail_smoke(
          QStringLiteral("revocation probe grant could not be restored"));
      return;
    }
    is_revocation_probe_active_ = true;
    is_capture_job_active_ = true;
    start_recording(target);
  }

  void finish_hidden_capture_checks(CaptureTarget target) {
    is_hidden_capture_revocation_rejected_ =
        is_revocation_probe_cancelled_ &&
        revocation_probe_frames_before_ == revocation_probe_frames_after_ &&
        revocation_probe_frames_before_names_ ==
            revocation_probe_frames_after_names_ &&
        revocation_probe_frames_before_ > 0 &&
        !QFileInfo(revocation_probe_recording_path_).exists();
    is_selected_tab_granted(*target.policy);
    const bool is_allowed_before_session_end = is_capture_allowed(target);
    end_session(*target.policy);
    is_hidden_capture_session_end_rejected_ =
        is_allowed_before_session_end && is_screenshot_rejected(target);
    finish_smoke_if_ready();
  }

  enum class ManualSmokeStep {
    WaitingForInitialLoads,
    WaitingForNavigation,
    WaitingForReload,
    WaitingForScreenshot,
    WaitingForRecording,
    WaitingForDecode,
    WaitingForEncoderRevocation,
    WaitingForRevocation,
    Finished,
  };

  void manual_smoke_failed(const QString &reason) {
    if (manual_smoke_step_ == ManualSmokeStep::Finished) {
      return;
    }
    manual_smoke_failure_ = reason;
    finish_manual_smoke();
  }

  void advance_manual_smoke_after_load(std::uint64_t id, bool is_ok) {
    if (!is_ok) {
      manual_smoke_failed(QStringLiteral("manual fixture load failed"));
      return;
    }
    if (manual_smoke_step_ == ManualSmokeStep::WaitingForInitialLoads) {
      if (tab_for(1) == nullptr || tab_for(2) == nullptr ||
          !tab_for(1)->last_committed_url.isValid() ||
          !tab_for(2)->last_committed_url.isValid()) {
        return;
      }
      manual_tab_one_url_ = tab_for(1)->last_committed_url;
      manual_tab_two_url_ = smoke_url(QStringLiteral("manual-tab-2"));
      select_tab(2, true);
      address_->setText(manual_tab_two_url_.toString());
      manual_smoke_step_ = ManualSmokeStep::WaitingForNavigation;
      navigate_action_->trigger();
      return;
    }
    if (id != 2 || tab_for(2) == nullptr) {
      return;
    }
    if (manual_smoke_step_ == ManualSmokeStep::WaitingForNavigation) {
      is_manual_selected_reload_verified_ =
          tab_for(2)->last_committed_url == manual_tab_two_url_ &&
          tab_for(1)->last_committed_url == manual_tab_one_url_;
      if (!is_manual_selected_reload_verified_) {
        manual_smoke_failed(
            QStringLiteral("manual selected navigation failed"));
        return;
      }
      manual_smoke_step_ = ManualSmokeStep::WaitingForReload;
      reload_action_->trigger();
      return;
    }
    if (manual_smoke_step_ == ManualSmokeStep::WaitingForReload) {
      is_manual_selected_reload_verified_ =
          is_manual_selected_reload_verified_ &&
          tab_for(2)->last_committed_url == manual_tab_two_url_ &&
          tab_for(1)->last_committed_url == manual_tab_one_url_;
      if (!is_manual_selected_reload_verified_) {
        manual_smoke_failed(QStringLiteral("manual selected reload failed"));
        return;
      }
      grant_action_->trigger();
      manual_smoke_step_ = ManualSmokeStep::WaitingForScreenshot;
      screenshot_action_->trigger();
    }
  }

  void advance_manual_smoke_after_screenshot(const CaptureTarget &target,
                                             const QString &path,
                                             const QPixmap &image) {
    if (manual_smoke_step_ != ManualSmokeStep::WaitingForScreenshot) {
      return;
    }
    is_manual_selected_screenshot_verified_ =
        target.target.tab_id == 2 && target.view == tab_for(2)->view &&
        tab_for(2)->last_committed_url == manual_tab_two_url_ &&
        is_fixture_pixels_present(image) && QFileInfo(path).size() > 0;
    if (!is_manual_selected_screenshot_verified_) {
      manual_smoke_failed(QStringLiteral("manual selected screenshot failed"));
      return;
    }
    capture_native_controls();
    is_manual_native_controls_verified_ =
        is_native_controls_written_ && window_->isVisible();
    if (!is_manual_native_controls_verified_) {
      manual_smoke_failed(
          QStringLiteral("manual native controls capture failed"));
      return;
    }
    manual_smoke_step_ = ManualSmokeStep::WaitingForRecording;
    record_action_->trigger();
  }

  void advance_manual_smoke_after_recording(const CaptureTarget &target,
                                            const QString &path) {
    if (manual_smoke_step_ != ManualSmokeStep::WaitingForRecording ||
        target.target.tab_id != 2) {
      manual_smoke_failed(QStringLiteral("manual recording target changed"));
      return;
    }
    manual_recording_path_ = path;
    manual_smoke_step_ = ManualSmokeStep::WaitingForDecode;
    recording_process_ = std::make_unique<QProcess>();
    QObject::connect(recording_process_.get(), &QProcess::errorOccurred,
                     window_.get(), [this](QProcess::ProcessError error) {
                       if (error == QProcess::FailedToStart) {
                         manual_smoke_failed(QStringLiteral(
                             "manual recording decoder is unavailable"));
                       }
                     });
    QObject::connect(
        recording_process_.get(), &QProcess::finished, window_.get(),
        [this](int exit_code, QProcess::ExitStatus exit_status) {
          is_manual_recording_decodable_ =
              exit_status == QProcess::NormalExit && exit_code == 0 &&
              QFileInfo(manual_recording_path_).size() > 0;
          if (!is_manual_recording_decodable_) {
            manual_smoke_failed(
                QStringLiteral("manual recording is not decodable"));
            return;
          }
          manual_smoke_step_ = ManualSmokeStep::WaitingForEncoderRevocation;
          grant_action_->trigger();
          record_action_->trigger();
        });
    recording_process_->start(QStringLiteral("ffmpeg"),
                              {QStringLiteral("-v"), QStringLiteral("error"),
                               QStringLiteral("-i"), manual_recording_path_,
                               QStringLiteral("-f"), QStringLiteral("null"),
                               QStringLiteral("-")});
  }

  void finish_manual_encoder_revocation(const CaptureTarget &target) {
    if (manual_smoke_step_ != ManualSmokeStep::WaitingForEncoderRevocation) {
      return;
    }
    manual_encoder_revocation_frames_after_ =
        recording_frame_names(manual_encoder_revocation_recording_directory_);
    is_manual_encoder_revocation_verified_ =
        target.target.tab_id == 2 && is_manual_encoder_started_ &&
        !is_capture_job_active_ && !is_tab_granted(*policy_, 2) &&
        window_->isVisible() &&
        manual_encoder_revocation_frames_before_.size() == 5 &&
        manual_encoder_revocation_frames_before_ ==
            manual_encoder_revocation_frames_after_ &&
        !QFileInfo(manual_encoder_revocation_recording_path_).exists() &&
        QFileInfo(manual_encoder_revocation_pending_recording_path_).exists();
    if (!is_manual_encoder_revocation_verified_) {
      manual_smoke_failed(QStringLiteral("manual encoder revocation failed"));
      return;
    }
    manual_smoke_step_ = ManualSmokeStep::WaitingForRevocation;
    grant_action_->trigger();
    is_manual_revocation_requested_ = true;
    record_action_->trigger();
  }

  void finish_manual_revocation(const CaptureTarget &target) {
    if (manual_smoke_step_ != ManualSmokeStep::WaitingForRevocation) {
      return;
    }
    manual_revocation_frames_after_ =
        recording_frame_names(manual_revocation_recording_directory_);
    is_manual_revocation_verified_ =
        target.target.tab_id == 2 && !is_capture_job_active_ &&
        !is_tab_granted(*policy_, 2) &&
        !manual_revocation_frames_before_.isEmpty() &&
        manual_revocation_frames_before_ == manual_revocation_frames_after_ &&
        !QFileInfo(manual_revocation_recording_path_).exists();
    if (!is_manual_revocation_verified_) {
      manual_smoke_failed(QStringLiteral("manual recording revocation failed"));
      return;
    }
    finish_manual_smoke();
  }

  void finish_manual_smoke() {
    if (manual_smoke_step_ == ManualSmokeStep::Finished) {
      return;
    }
    is_manual_browser_open_verified_ = window_->isVisible() &&
                                       !is_smoke_finished_ &&
                                       manual_hidden_capture_entries_ == 0;
    const bool is_passed =
        manual_smoke_failure_.isEmpty() &&
        is_manual_selected_reload_verified_ &&
        is_manual_selected_screenshot_verified_ &&
        is_manual_recording_decodable_ &&
        is_manual_encoder_revocation_verified_ &&
        is_manual_revocation_verified_ && is_manual_native_controls_verified_ &&
        is_manual_browser_open_verified_ && !is_capture_job_active_;
    QJsonObject cases;
    cases.insert(QStringLiteral("manual_selected_tab_reload"),
                 is_manual_selected_reload_verified_
                     ? QStringLiteral("passed")
                     : QStringLiteral("failed"));
    cases.insert(QStringLiteral("manual_selected_tab_screenshot"),
                 is_manual_selected_screenshot_verified_
                     ? QStringLiteral("passed")
                     : QStringLiteral("failed"));
    cases.insert(QStringLiteral("manual_visible_recording"),
                 is_manual_recording_decodable_ ? QStringLiteral("passed")
                                                : QStringLiteral("failed"));
    cases.insert(QStringLiteral("manual_recording_encoder_revocation"),
                 is_manual_encoder_revocation_verified_
                     ? QStringLiteral("passed")
                     : QStringLiteral("failed"));
    cases.insert(QStringLiteral("manual_recording_revocation"),
                 is_manual_revocation_verified_ ? QStringLiteral("passed")
                                                : QStringLiteral("failed"));
    cases.insert(QStringLiteral("manual_native_controls"),
                 is_manual_native_controls_verified_
                     ? QStringLiteral("passed")
                     : QStringLiteral("failed"));
    cases.insert(QStringLiteral("manual_browser_open_no_hidden_capture"),
                 is_manual_browser_open_verified_ ? QStringLiteral("passed")
                                                  : QStringLiteral("failed"));
    QJsonObject record;
    record.insert(QStringLiteral("candidate"), QStringLiteral("qt"));
    record.insert(QStringLiteral("outcome"), is_passed
                                                 ? QStringLiteral("passed")
                                                 : QStringLiteral("failed"));
    record.insert(QStringLiteral("reason"), manual_smoke_failure_);
    record.insert(QStringLiteral("cases"), cases);
    record.insert(QStringLiteral("selected_tab"),
                  static_cast<qint64>(selected_tab_id()));
    record.insert(QStringLiteral("tab_one_url"),
                  manual_tab_one_url_.toString());
    record.insert(QStringLiteral("tab_two_url"),
                  manual_tab_two_url_.toString());
    record.insert(QStringLiteral("recording_path"), manual_recording_path_);
    record.insert(QStringLiteral("encoder_revocation_recording_path"),
                  manual_encoder_revocation_recording_path_);
    record.insert(QStringLiteral("encoder_revocation_pending_recording_path"),
                  manual_encoder_revocation_pending_recording_path_);
    record.insert(
        QStringLiteral("encoder_revocation_pending_recording_exists"),
        QFileInfo(manual_encoder_revocation_pending_recording_path_).exists());
    record.insert(QStringLiteral("encoder_revocation_frames_before"),
                  manual_encoder_revocation_frames_before_);
    record.insert(QStringLiteral("encoder_revocation_frames_after"),
                  manual_encoder_revocation_frames_after_);
    record.insert(QStringLiteral("revocation_recording_path"),
                  manual_revocation_recording_path_);
    record.insert(QStringLiteral("revocation_pending_recording_path"),
                  manual_revocation_pending_recording_path_);
    record.insert(
        QStringLiteral("revocation_pending_recording_exists"),
        QFileInfo(manual_revocation_pending_recording_path_).exists());
    record.insert(QStringLiteral("revocation_frames_before"),
                  manual_revocation_frames_before_);
    record.insert(QStringLiteral("revocation_frames_after"),
                  manual_revocation_frames_after_);
    record.insert(QStringLiteral("browser_open_before_driver_exit"),
                  window_->isVisible());
    record.insert(QStringLiteral("native_toolbar_path"),
                  QDir(evidence_directory_)
                      .filePath(QStringLiteral("native-toolbar.png")));
    record.insert(
        QStringLiteral("native_tabs_path"),
        QDir(evidence_directory_).filePath(QStringLiteral("native-tabs.png")));
    record.insert(QStringLiteral("smoke_shutdown_started"), is_smoke_finished_);
    record.insert(QStringLiteral("hidden_capture_entries"),
                  manual_hidden_capture_entries_);
    QFile file(QDir(evidence_directory_)
                   .filePath(QStringLiteral("manual-smoke.json")));
    const QByteArray encoded =
        QJsonDocument(record).toJson(QJsonDocument::Compact);
    const bool is_written =
        file.open(QIODevice::WriteOnly | QIODevice::Truncate) &&
        file.write(encoded) == encoded.size() && file.flush();
    manual_smoke_step_ = ManualSmokeStep::Finished;
    smoke_deadline_.stop();
    exit_code_ = is_passed && is_written ? 0 : 1;
    if (is_passed && !is_written) {
      qWarning("manual smoke checks passed; manifest write failed");
    }
    QTimer::singleShot(0, application_.get(),
                       [this] { application_->exit(exit_code_); });
  }

  void verify_final_invalidation() {
    select_tab(2, true);
    grant_selected();
    const PageTarget closed_target = target_for(*policy_, 2);
    const bool is_tab_target_allowed =
        is_control_allowed(*policy_, closed_target);
    destroy_tab(2);
    is_tab_invalidated_ =
        is_tab_target_allowed && !is_control_allowed(*policy_, closed_target);
    select_tab(1, true);
    grant_selected();
    const PageTarget session_target = target_for(*policy_, 1);
    const bool is_session_target_allowed =
        is_control_allowed(*policy_, session_target);
    end_session(*policy_);
    is_session_invalidated_ = is_session_target_allowed &&
                              !is_control_allowed(*policy_, session_target);
    CaptureTarget invalidated_target = visible_capture_target();
    is_capture_session_end_rejected_ =
        is_screenshot_rejected(invalidated_target);
  }

  void finish_smoke_if_ready() {
    if (is_smoke_finished_ || !is_visible_input_verified_ ||
        !is_storage_isolated_ ||
        !std::all_of(is_screenshot_written_.begin(),
                     is_screenshot_written_.end(),
                     [](bool is_written) { return is_written; }) ||
        !std::all_of(is_recording_written_.begin(), is_recording_written_.end(),
                     [](bool is_written) { return is_written; }) ||
        !is_native_controls_written_) {
      return;
    }
    verify_final_invalidation();
    const bool is_policy_verified =
        is_first_target_allowed_ && is_stale_target_rejected_ &&
        is_fresh_target_allowed_ && is_revoked_target_rejected_ &&
        is_unknown_tab_rejected_ && is_nonfixture_navigation_rejected_ &&
        is_discard_observed_ && is_split_exercised_ &&
        is_split_geometry_verified_ && is_hidden_host_configuration_verified_ &&
        std::all_of(is_split_engine_snapshot_written_.begin(),
                    is_split_engine_snapshot_written_.end(),
                    [](bool is_written) { return is_written; }) &&
        std::all_of(is_split_document_fixture_verified_.begin(),
                    is_split_document_fixture_verified_.end(),
                    [](bool is_verified) { return is_verified; }) &&
        is_tab_invalidated_ && is_session_invalidated_ &&
        is_capture_wrong_target_rejected_ && is_capture_wrong_page_rejected_ &&
        is_native_control_selected_tab_rejected_ &&
        is_capture_revocation_rejected_ && is_capture_session_end_rejected_ &&
        is_capture_reentry_rejected_ &&
        std::all_of(is_hidden_capture_grant_denied_.begin(),
                    is_hidden_capture_grant_denied_.end(),
                    [](bool is_rejected) { return is_rejected; }) &&
        is_hidden_capture_revocation_rejected_ &&
        is_hidden_capture_session_end_rejected_;
    if (!is_policy_verified) {
      fail_smoke(QStringLiteral("policy or lifecycle case failed"));
      return;
    }
    complete_smoke(QStringLiteral("passed"), QString());
  }

  void fail_smoke(const QString &reason) {
    if (is_smoke_finished_) {
      return;
    }
    complete_smoke(QStringLiteral("failed"), reason);
  }

  void block_smoke(const QString &reason) {
    if (is_smoke_finished_) {
      return;
    }
    complete_smoke(QStringLiteral("blocked"), reason);
  }

  void complete_smoke(const QString &outcome, const QString &reason) {
    is_smoke_finished_ = true;
    smoke_deadline_.stop();
    const bool is_evidence_written = is_smoke_manifest_written(outcome, reason);
    exit_code_ =
        outcome == QStringLiteral("passed") && is_evidence_written ? 0 : 1;
    QTimer::singleShot(0, application_.get(), [this] { application_->quit(); });
  }

  bool is_smoke_manifest_written(const QString &outcome,
                                 const QString &reason) const {
    QJsonObject cases;
    cases.insert(QStringLiteral("visible_load"),
                 is_visible_input_verified_ ? QStringLiteral("passed")
                                            : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("stale_target"),
                 is_stale_target_rejected_ ? QStringLiteral("passed")
                                           : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("revocation"),
                 is_revoked_target_rejected_ ? QStringLiteral("passed")
                                             : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("unknown_tab"),
                 is_unknown_tab_rejected_ ? QStringLiteral("passed")
                                          : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("nonfixture_navigation"),
                 is_nonfixture_navigation_rejected_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("certificate_policy"),
                 QStringLiteral("blocked: fixture has no TLS failure route"));
    cases.insert(QStringLiteral("system_permission_independence"),
                 QStringLiteral("unverified: no system permission approval"));
    cases.insert(QStringLiteral("discard_reload"),
                 is_discard_observed_ ? QStringLiteral("passed")
                                      : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("split"),
                 is_split_exercised_ && is_split_geometry_verified_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("split_engine_snapshots"),
                 std::all_of(is_split_engine_snapshot_written_.begin(),
                             is_split_engine_snapshot_written_.end(),
                             [](bool is_written) { return is_written; })
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("hidden_host_configuration"),
                 is_hidden_host_configuration_verified_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("storage"), is_storage_isolated_
                                                ? QStringLiteral("passed")
                                                : QStringLiteral("unreached"));
    for (const QString &profile :
         {QStringLiteral("visible"), QStringLiteral("hidden_alpha"),
          QStringLiteral("hidden_beta")}) {
      const int index =
          profile == QStringLiteral("visible")
              ? 0
              : (profile == QStringLiteral("hidden_alpha") ? 1 : 2);
      cases.insert(profile + QStringLiteral("_screenshot"),
                   is_screenshot_written_[static_cast<std::size_t>(index)]
                       ? QStringLiteral("passed")
                       : QStringLiteral("unreached"));
      cases.insert(profile + QStringLiteral("_recording"),
                   is_recording_written_[static_cast<std::size_t>(index)]
                       ? QStringLiteral("passed")
                       : QStringLiteral("unreached"));
    }
    cases.insert(QStringLiteral("native_controls"),
                 is_native_controls_written_ ? QStringLiteral("passed")
                                             : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("hidden_grant_denial"),
                 std::all_of(is_hidden_capture_grant_denied_.begin(),
                             is_hidden_capture_grant_denied_.end(),
                             [](bool is_rejected) { return is_rejected; })
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("hidden_revocation"),
                 is_hidden_capture_revocation_rejected_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("revocation_recording_callback"),
                 is_revocation_probe_cancelled_ &&
                         revocation_probe_frames_before_ > 0 &&
                         revocation_probe_frames_before_ ==
                             revocation_probe_frames_after_ &&
                         revocation_probe_frames_before_names_ ==
                             revocation_probe_frames_after_names_ &&
                         !QFileInfo(revocation_probe_recording_path_).exists()
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("hidden_session_end"),
                 is_hidden_capture_session_end_rejected_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("capture_wrong_original_target"),
                 is_capture_wrong_target_rejected_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("capture_wrong_page_target"),
                 is_capture_wrong_page_rejected_ ? QStringLiteral("passed")
                                                 : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("capture_revocation"),
                 is_capture_revocation_rejected_ ? QStringLiteral("passed")
                                                 : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("capture_session_end"),
                 is_capture_session_end_rejected_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("native_control_selected_tab"),
                 is_native_control_selected_tab_rejected_
                     ? QStringLiteral("passed")
                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("capture_reentry"),
                 is_capture_reentry_rejected_ ? QStringLiteral("passed")
                                              : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("tab_invalidation"),
                 is_tab_invalidated_ ? QStringLiteral("passed")
                                     : QStringLiteral("unreached"));
    cases.insert(QStringLiteral("session_invalidation"),
                 is_session_invalidated_ ? QStringLiteral("passed")
                                         : QStringLiteral("unreached"));
    QJsonObject record;
    record.insert(QStringLiteral("candidate"), QStringLiteral("qt"));
    record.insert(QStringLiteral("outcome"), outcome);
    record.insert(QStringLiteral("reason"), reason);
    record.insert(QStringLiteral("cases"), cases);
    QJsonArray storage_results;
    for (const QString &result : storage_results_) {
      storage_results.append(result);
    }
    record.insert(QStringLiteral("storage_results"), storage_results);
    record.insert(QStringLiteral("split_geometry"), split_geometry_);
    record.insert(QStringLiteral("split_engine_snapshots"),
                  split_engine_snapshots_);
    record.insert(QStringLiteral("split_document_evidence"),
                  split_document_evidence_);
    record.insert(QStringLiteral("split_target_evidence"),
                  split_target_evidence_);
    record.insert(QStringLiteral("hidden_hosts"), hidden_hosts());
    QJsonObject revocation_probe;
    revocation_probe.insert(
        QStringLiteral("target_tab_id"),
        static_cast<qint64>(revocation_probe_target_.tab_id));
    revocation_probe.insert(
        QStringLiteral("target_navigation_generation"),
        static_cast<qint64>(revocation_probe_target_.navigation_generation));
    revocation_probe.insert(QStringLiteral("recording_directory"),
                            revocation_probe_recording_directory_);
    revocation_probe.insert(QStringLiteral("recording_path"),
                            revocation_probe_recording_path_);
    revocation_probe.insert(QStringLiteral("frames_before"),
                            revocation_probe_frames_before_);
    revocation_probe.insert(QStringLiteral("frame_names_before"),
                            revocation_probe_frames_before_names_);
    revocation_probe.insert(QStringLiteral("frames_after"),
                            revocation_probe_frames_after_);
    revocation_probe.insert(QStringLiteral("frame_names_after"),
                            revocation_probe_frames_after_names_);
    revocation_probe.insert(QStringLiteral("cancelled_in_recorder_callback"),
                            is_revocation_probe_cancelled_);
    revocation_probe.insert(
        QStringLiteral("completed_movie_exists"),
        QFileInfo(revocation_probe_recording_path_).exists());
    record.insert(QStringLiteral("revocation_probe"), revocation_probe);
    const QString path =
        QDir(evidence_directory_).filePath(QStringLiteral("smoke.json"));
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
      return false;
    }
    const QByteArray encoded =
        QJsonDocument(record).toJson(QJsonDocument::Compact);
    return file.write(encoded) == encoded.size() && file.flush();
  }

  QUrl smoke_url(const QString &value) const {
    QUrl url = fixture_origin_.resolved(QUrl(QStringLiteral("smoke.html")));
    url.setQuery(QStringLiteral("value=%1").arg(value));
    return url;
  }

  QUrl frame_url() const {
    return fixture_origin_.resolved(QUrl(QStringLiteral("frame.html")));
  }

  std::uint64_t selected_tab_id() const {
    const int index = tabs_->currentIndex();
    if (index < 0 || static_cast<std::size_t>(index) >= tab_records_.size()) {
      return 0;
    }
    return tab_records_[static_cast<std::size_t>(index)].id;
  }

  Tab *tab_for(std::uint64_t id) {
    for (Tab &tab : tab_records_) {
      if (tab.id == id) {
        return &tab;
      }
    }
    return nullptr;
  }

  const Tab *tab_for(std::uint64_t id) const {
    for (const Tab &tab : tab_records_) {
      if (tab.id == id) {
        return &tab;
      }
    }
    return nullptr;
  }

  void destroy_tab(std::uint64_t id) {
    const auto iterator =
        std::find_if(tab_records_.begin(), tab_records_.end(),
                     [id](const Tab &tab) { return tab.id == id; });
    if (iterator == tab_records_.end()) {
      return;
    }
    tabs_->removeTab(tabs_->indexOf(iterator->view));
    iterator->view->setParent(nullptr);
    delete iterator->view;
    iterator->view = nullptr;
    is_tab_closed(*policy_, iterator->id);
    iterator->page.reset();
    tab_records_.erase(iterator);
  }

  void destroy_tabs() {
    while (!tab_records_.empty()) {
      destroy_tab(tab_records_.back().id);
    }
  }

  void destroy_hidden_session(HiddenSession &session) {
    if (session.view != nullptr) {
      session.view->setParent(nullptr);
    }
    session.host.reset();
    session.view.reset();
    session.page.reset();
    session.policy.reset();
    session.interceptor.reset();
    session.profile.reset();
  }

  void status(const QString &message, std::uint64_t id) {
    window_->statusBar()->showMessage(
        QStringLiteral("Tab %1: %2").arg(id).arg(message));
  }

  rust::Box<BrowserPolicy> policy_;
  std::string application_name_ = "qt-smoke";
  int argc_ = 1;
  std::array<char *, 2> argv_;
  std::unique_ptr<QApplication> application_;
  std::unique_ptr<QMainWindow> window_;
  QLineEdit *address_ = nullptr;
  QToolBar *toolbar_ = nullptr;
  QAction *navigate_action_ = nullptr;
  QAction *reload_action_ = nullptr;
  QAction *grant_action_ = nullptr;
  QAction *revoke_action_ = nullptr;
  QAction *screenshot_action_ = nullptr;
  QAction *record_action_ = nullptr;
  QAction *split_action_ = nullptr;
  QSplitter *splitter_ = nullptr;
  QTabWidget *tabs_ = nullptr;
  std::unique_ptr<QWebEngineProfile> visible_profile_;
  std::unique_ptr<FixtureRequestInterceptor> visible_interceptor_;
  HiddenSession hidden_one_;
  HiddenSession hidden_two_;
  std::vector<Tab> tab_records_;
  QUrl fixture_origin_;
  QJsonObject split_geometry_;
  QJsonArray split_engine_snapshots_;
  QJsonArray split_document_evidence_;
  QJsonArray split_target_evidence_;
  QString evidence_directory_;
  QString recording_directory_;
  QString revocation_probe_recording_directory_;
  QString revocation_probe_recording_path_;
  std::array<QString, 3> storage_results_;
  std::unique_ptr<QProcess> recording_process_;
  QSet<QByteArray> recording_frame_hashes_;
  QTimer smoke_deadline_;
  PageTarget stale_target_{};
  PageTarget fresh_target_{};
  int recording_frames_remaining_ = 0;
  int storage_read_count_ = 0;
  int exit_code_ = 1;
  bool is_interactive_ = false;
  bool is_manual_smoke_ = false;
  ManualSmokeStep manual_smoke_step_ = ManualSmokeStep::WaitingForInitialLoads;
  QString manual_smoke_failure_;
  QUrl manual_tab_one_url_;
  QUrl manual_tab_two_url_;
  QString manual_recording_path_;
  QString manual_encoder_revocation_recording_directory_;
  QString manual_encoder_revocation_recording_path_;
  QString manual_encoder_revocation_pending_recording_path_;
  QJsonArray manual_encoder_revocation_frames_before_;
  QJsonArray manual_encoder_revocation_frames_after_;
  QString manual_revocation_recording_directory_;
  QString manual_revocation_recording_path_;
  QString manual_revocation_pending_recording_path_;
  QJsonArray manual_revocation_frames_before_;
  QJsonArray manual_revocation_frames_after_;
  bool is_manual_selected_reload_verified_ = false;
  bool is_manual_selected_screenshot_verified_ = false;
  bool is_manual_recording_decodable_ = false;
  bool is_manual_encoder_started_ = false;
  bool is_manual_encoder_revocation_verified_ = false;
  bool is_manual_revocation_requested_ = false;
  bool is_manual_revocation_verified_ = false;
  bool is_manual_native_controls_verified_ = false;
  bool is_manual_browser_open_verified_ = false;
  int manual_hidden_capture_entries_ = 0;
  bool is_waiting_for_initial_load_ = false;
  bool is_waiting_for_navigation_ = false;
  bool is_waiting_for_discard_ = false;
  bool is_waiting_for_reload_ = false;
  bool is_hidden_one_loaded_ = false;
  bool is_hidden_two_loaded_ = false;
  bool is_first_target_allowed_ = false;
  bool is_stale_target_rejected_ = false;
  bool is_fresh_target_allowed_ = false;
  bool is_revoked_target_rejected_ = false;
  bool is_unknown_tab_rejected_ = false;
  bool is_nonfixture_navigation_rejected_ = false;
  bool is_discard_observed_ = false;
  bool is_split_exercised_ = false;
  bool is_split_geometry_verified_ = false;
  bool is_hidden_host_configuration_verified_ = false;
  std::array<bool, 2> is_split_engine_snapshot_written_{};
  std::array<bool, 2> is_split_document_fixture_verified_{};
  bool is_visible_input_verified_ = false;
  bool is_storage_isolated_ = false;
  std::array<bool, 3> is_screenshot_written_{};
  std::array<bool, 3> is_recording_written_{};
  int capture_sequence_ = 0;
  bool is_capture_job_active_ = false;
  bool is_native_controls_written_ = false;
  bool is_native_control_probe_active_ = false;
  bool is_native_control_capture_result_observed_ = false;
  bool is_native_control_capture_target_observed_ = false;
  std::uint64_t native_control_capture_target_id_ = 0;
  CaptureStartResult native_control_capture_result_ =
      CaptureStartResult::Started;
  bool is_native_control_selected_tab_rejected_ = false;
  bool is_capture_wrong_target_rejected_ = false;
  bool is_capture_wrong_page_rejected_ = false;
  bool is_capture_revocation_rejected_ = false;
  bool is_capture_session_end_rejected_ = false;
  bool is_capture_reentry_rejected_ = false;
  std::array<bool, 2> is_hidden_capture_grant_denied_{};
  bool is_hidden_capture_revocation_rejected_ = false;
  bool is_hidden_capture_session_end_rejected_ = false;
  bool is_revocation_probe_active_ = false;
  bool is_revocation_probe_request_pending_ = false;
  bool is_revocation_probe_cancelled_ = false;
  PageTarget revocation_probe_target_{};
  QJsonArray revocation_probe_frames_before_names_;
  QJsonArray revocation_probe_frames_after_names_;
  int revocation_probe_frames_before_ = 0;
  int revocation_probe_frames_after_ = 0;
  bool is_smoke_finished_ = false;
  bool is_tab_invalidated_ = false;
  bool is_session_invalidated_ = false;
};

QtSmokeAdapter::QtSmokeAdapter(rust::Box<BrowserPolicy> policy)
    : impl_(std::make_unique<QtSmokeAdapterImpl>(std::move(policy))) {}

QtSmokeAdapter::~QtSmokeAdapter() = default;

int QtSmokeAdapter::run_smoke(rust::Str fixture_base_address,
                              rust::Str evidence_directory, bool is_interactive,
                              bool is_manual_smoke) {
  return impl_->run_smoke(fixture_base_address, evidence_directory,
                          is_interactive, is_manual_smoke);
}

std::unique_ptr<QtSmokeAdapter> new_adapter(rust::Box<BrowserPolicy> policy) {
  return std::make_unique<QtSmokeAdapter>(std::move(policy));
}

int run_smoke(QtSmokeAdapter &adapter, rust::Str fixture_base_address,
              rust::Str evidence_directory, bool is_interactive,
              bool is_manual_smoke) {
  return adapter.run_smoke(fixture_base_address, evidence_directory,
                           is_interactive, is_manual_smoke);
}

} // namespace qt_smoke
