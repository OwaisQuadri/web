#include "memory.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QDir>
#include <QtCore/QElapsedTimer>
#include <QtCore/QFile>
#include <QtCore/QFileInfo>
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QStringList>
#include <QtCore/QThread>
#include <QtCore/QUrl>
#include <QtCore/QVariant>
#include <QtWebEngineCore/QWebEnginePage>
#include <QtWebEngineCore/QWebEngineProfile>
#include <QtWebEngineWidgets/QWebEngineView>
#include <QtWidgets/QApplication>
#include <QtWidgets/QWidget>

#include <memory>
#include <stdexcept>
#include <vector>

namespace qt_smoke {
namespace {
constexpr int kWidth = 800;
constexpr int kHeight = 600;
constexpr qint64 kDeadlineMilliseconds = 60000;

struct Arguments final {
  QString origin;
  QString evidence;
  QString scenario;
  int tab_count;
  QString background_state;
  QString layout;
};

struct Record final {
  QString role;
  QUrl url;
  std::unique_ptr<QWebEnginePage> page;
  std::unique_ptr<QWebEngineView> view;
  QWebEnginePage::LifecycleState lifecycle =
      QWebEnginePage::LifecycleState::Active;
  bool is_discard_observed = false;
};

QString to_qstring(rust::Str value) {
  return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

bool is_valid_arguments(const Arguments &arguments) {
  const bool is_allowed =
      (arguments.background_state == QStringLiteral("loaded") &&
       arguments.layout == QStringLiteral("single") &&
       (arguments.tab_count == 7 || arguments.tab_count == 12 ||
        arguments.tab_count == 20)) ||
      (arguments.background_state == QStringLiteral("unloaded") &&
       arguments.layout == QStringLiteral("single") &&
       arguments.tab_count == 20) ||
      (arguments.background_state == QStringLiteral("loaded") &&
       arguments.layout == QStringLiteral("split") &&
       arguments.tab_count == 7);
  const QString expected = QStringLiteral("%1-%2-%3")
                               .arg(arguments.background_state,
                                    arguments.layout)
                               .arg(arguments.tab_count);
  return is_allowed && arguments.scenario == expected;
}

Arguments parse_arguments(rust::Str fixture_origin,
                          rust::Str evidence_directory,
                          rust::Str scenario_id, rust::Str tab_count,
                          rust::Str background_state, rust::Str layout) {
  bool is_count_valid = false;
  const int count = to_qstring(tab_count).toInt(&is_count_valid);
  Arguments arguments{to_qstring(fixture_origin),
                      to_qstring(evidence_directory),
                      to_qstring(scenario_id),
                      count,
                      to_qstring(background_state),
                      to_qstring(layout)};
  if (!is_count_valid || !is_valid_arguments(arguments)) {
    throw std::runtime_error(
        "accepted combinations are loaded-single-7/12/20, "
        "unloaded-single-20, and loaded-split-7");
  }
  return arguments;
}

void reject_existing(const QString &directory) {
  for (const QString &name : {QStringLiteral("memory-ready-v1.json"),
                              QStringLiteral("memory-shutdown-v1.json"),
                              QStringLiteral("memory-ready-v1.json.pending"),
                              QStringLiteral(
                                  "memory-shutdown-v1.json.pending")}) {
    if (QFileInfo::exists(QDir(directory).filePath(name))) {
      throw std::runtime_error("memory evidence already exists");
    }
  }
}

void write_atomic(const QString &path, const QJsonObject &document) {
  const QString pending = path + QStringLiteral(".pending");
  const QByteArray encoded =
      QJsonDocument(document).toJson(QJsonDocument::Compact);
  QFile file(pending);
  if (!file.open(QIODevice::WriteOnly | QIODevice::NewOnly)) {
    throw std::runtime_error("memory evidence pending file already exists");
  }
  if (file.write(encoded) != encoded.size() || !file.flush()) {
    throw std::runtime_error("memory evidence write failed");
  }
  file.close();
  if (!file.rename(path)) {
    throw std::runtime_error("memory evidence destination already exists");
  }
}

QString expected_kind(const QUrl &url) {
  if (url.path().endsWith(QStringLiteral("images.html"))) {
    return QStringLiteral("images");
  }
  if (url.path().endsWith(QStringLiteral("application.html"))) {
    return QStringLiteral("application");
  }
  return QStringLiteral("article");
}

bool is_payload(const QVariant &value, const QString &kind,
                int action_count) {
  QJsonParseError error;
  const QJsonDocument document =
      QJsonDocument::fromJson(value.toString().toUtf8(), &error);
  if (error.error != QJsonParseError::NoError || !document.isObject()) {
    return false;
  }
  const QJsonObject object = document.object();
  const QJsonObject details = object.value(QStringLiteral("details")).toObject();
  const bool is_content_ready =
      (kind == QStringLiteral("article") &&
       details.value(QStringLiteral("paragraphs")).toInt(-1) == 100) ||
      (kind == QStringLiteral("images") &&
       details.value(QStringLiteral("images")).toInt(-1) == 3 &&
       details.value(QStringLiteral("pixels")).toInt(-1) == 2359296) ||
      (kind == QStringLiteral("application") &&
       details.value(QStringLiteral("rows")).toInt(-1) == 2000);
  return object.value(QStringLiteral("ready")).isBool() &&
         object.value(QStringLiteral("ready")).toBool() && is_content_ready &&
         object.value(QStringLiteral("kind")).isString() &&
         object.value(QStringLiteral("kind")).toString() == kind &&
         object.value(QStringLiteral("actionCount")).isDouble() &&
         object.value(QStringLiteral("actionCount")).toInt(-1) == action_count;
}

struct Evaluation final {
  bool is_done = false;
  QVariant result;
};

bool is_evaluated(QWebEnginePage *page, const QString &script,
                  QVariant *result, QElapsedTimer *clock) {
  auto evaluation = std::make_shared<Evaluation>();
  page->runJavaScript(
      script, [evaluation](const QVariant &value) {
        evaluation->result = value;
        evaluation->is_done = true;
      });
  while (!evaluation->is_done && clock->elapsed() < kDeadlineMilliseconds) {
    QApplication::processEvents(QEventLoop::AllEvents, 20);
    QThread::msleep(5);
  }
  if (!evaluation->is_done) {
    return false;
  }
  *result = evaluation->result;
  return true;
}

void ready_and_act(Record &record, QElapsedTimer *clock) {
  const QString status = QStringLiteral(
      "(() => { const value = typeof window.webBenchmarkStatus === "
      "'function' ? window.webBenchmarkStatus() : null; return "
      "JSON.stringify(value); })()");
  QVariant result;
  while (clock->elapsed() < kDeadlineMilliseconds) {
    if (is_evaluated(record.page.get(), status, &result, clock) &&
        is_payload(result, expected_kind(record.url), 0)) {
      const QString act = QStringLiteral(
          "(() => { const value = typeof window.webBenchmarkAct === "
          "'function' ? window.webBenchmarkAct() : null; return "
          "JSON.stringify(value); })()");
      if (!is_evaluated(record.page.get(), act, &result, clock) ||
          !is_payload(result, expected_kind(record.url), 1)) {
        throw std::runtime_error(
            "webBenchmarkAct did not return typed actionCount=1");
      }
      return;
    }
    QApplication::processEvents(QEventLoop::AllEvents, 20);
    QThread::msleep(25);
  }
  throw std::runtime_error("60-second readiness deadline elapsed");
}

QJsonObject record_json(const Record &record) {
  QJsonObject object;
  object.insert(QStringLiteral("role"), record.role);
  object.insert(QStringLiteral("url"), record.url.toString());
  object.insert(
      QStringLiteral("lifecycle_state"),
      record.lifecycle == QWebEnginePage::LifecycleState::Discarded
          ? QStringLiteral("discarded")
          : QStringLiteral("active"));
  object.insert(QStringLiteral("logical_bounds"),
                QJsonObject{{QStringLiteral("width"), kWidth},
                            {QStringLiteral("height"), kHeight}});
  return object;
}

QJsonArray records_json(const std::vector<Record> &records) {
  QJsonArray array;
  for (const Record &record : records) {
    array.append(record_json(record));
  }
  return array;
}

void attach(QWidget *window, std::vector<Record> &visible,
            const QString &layout, int selected) {
  for (std::size_t index = 0; index < visible.size(); ++index) {
    QWebEngineView *view = visible[index].view.get();
    const bool is_shown = layout == QStringLiteral("split")
                              ? index < 2
                              : static_cast<int>(index) == selected;
    view->setParent(window);
    if (!is_shown) {
      view->hide();
      continue;
    }
    const int width =
        layout == QStringLiteral("split") ? kWidth / 2 : kWidth;
    const int x = layout == QStringLiteral("split") && index == 1
                      ? width
                      : 0;
    view->setGeometry(x, 0, width, kHeight);
    view->show();
  }
}

QUrl route(const QString &origin, const QString &name) {
  return QUrl(origin + QStringLiteral("/") + name, QUrl::StrictMode);
}

Record make_record(QWebEngineProfile *profile, const QString &role,
                   const QUrl &url) {
  Record record;
  record.role = role;
  record.url = url;
  record.page = std::make_unique<QWebEnginePage>(profile);
  record.view = std::make_unique<QWebEngineView>();
  record.view->setPage(record.page.get());
  record.view->resize(kWidth, kHeight);
  record.view->load(url);
  return record;
}

void destroy_records(std::vector<Record> *records) {
  for (Record &record : *records) {
    record.view.reset();
    record.page.reset();
  }
  records->clear();
}

void discard_backgrounds(std::vector<Record> *visible,
                         QElapsedTimer *clock) {
  for (std::size_t index = 1; index < visible->size(); ++index) {
    Record &record = visible->at(index);
    record.page->setLifecycleState(QWebEnginePage::LifecycleState::Discarded);
    while (record.page->lifecycleState() !=
               QWebEnginePage::LifecycleState::Discarded &&
           clock->elapsed() < kDeadlineMilliseconds) {
      QApplication::processEvents(QEventLoop::AllEvents, 20);
      QThread::msleep(5);
    }
    record.lifecycle = record.page->lifecycleState();
    record.is_discard_observed =
        record.lifecycle == QWebEnginePage::LifecycleState::Discarded;
    if (!record.is_discard_observed) {
      throw std::runtime_error("discarded lifecycle state was not observed");
    }
  }
}
}

int run_memory_smoke(rust::Str fixture_origin,
                     rust::Str evidence_directory, rust::Str scenario_id,
                     rust::Str tab_count, rust::Str background_state,
                     rust::Str layout) {
  const Arguments arguments =
      parse_arguments(fixture_origin, evidence_directory, scenario_id,
                      tab_count, background_state, layout);
  const QUrl origin(arguments.origin, QUrl::StrictMode);
  const bool is_loopback = origin.host() == QStringLiteral("localhost") ||
                           origin.host() == QStringLiteral("127.0.0.1") ||
                           origin.host() == QStringLiteral("::1");
  if (!origin.isValid() || origin.scheme() != QStringLiteral("http") ||
      !is_loopback || origin.port() <= 0 ||
      (!origin.path().isEmpty() && origin.path() != QStringLiteral("/"))) {
    throw std::runtime_error(
        "fixture origin must be a loopback http origin with a port");
  }
  if (QCoreApplication::instance() != nullptr) {
    throw std::runtime_error("Qt application already exists on this thread");
  }
  int application_argument_count = 1;
  char application_name[] = "qt-memory";
  char *application_arguments[] = {application_name, nullptr};
  QApplication application(application_argument_count, application_arguments);
  reject_existing(arguments.evidence);
  if (!QDir().mkpath(arguments.evidence)) {
    throw std::runtime_error("cannot create evidence directory");
  }
  QElapsedTimer clock;
  clock.start();
  QWidget window;
  window.setWindowTitle(QStringLiteral("Qt memory benchmark"));
  window.setGeometry(100, 100, kWidth, kHeight);

  auto visible_profile = std::make_unique<QWebEngineProfile>();
  if (!visible_profile->isOffTheRecord()) {
    throw std::runtime_error("visible profile is not off the record");
  }

  std::vector<Record> visible;
  const QStringList names{QStringLiteral("article.html"),
                          QStringLiteral("images.html"),
                          QStringLiteral("application.html")};
  for (int index = 0; index < arguments.tab_count; ++index) {
    visible.push_back(make_record(
        visible_profile.get(),
        index == 0 ? QStringLiteral("visible")
                   : QStringLiteral("background"),
        route(arguments.origin, names.at(index % names.size()))));
  }

  auto headless_first = std::make_unique<QWebEngineProfile>();
  auto headless_second = std::make_unique<QWebEngineProfile>();
  if (!headless_first->isOffTheRecord() ||
      !headless_second->isOffTheRecord()) {
    throw std::runtime_error("headless profile is not off the record");
  }
  std::vector<Record> headless;
  headless.push_back(make_record(
      headless_first.get(), QStringLiteral("headless-a"),
      route(arguments.origin, QStringLiteral("article.html"))));
  headless.push_back(make_record(
      headless_second.get(), QStringLiteral("headless-b"),
      route(arguments.origin, QStringLiteral("application.html"))));

  attach(&window, visible, QStringLiteral("single"), 0);
  window.show();
  for (std::size_t index = 0; index < visible.size(); ++index) {
    attach(&window, visible, QStringLiteral("single"),
           static_cast<int>(index));
    ready_and_act(visible[index], &clock);
  }
  for (Record &record : headless) {
    ready_and_act(record, &clock);
  }
  attach(&window, visible, QStringLiteral("single"), 0);
  if (arguments.background_state == QStringLiteral("unloaded")) {
    discard_backgrounds(&visible, &clock);
  }
  attach(&window, visible, arguments.layout, 0);

  QJsonObject ready;
  ready.insert(QStringLiteral("schema_version"), 1);
  ready.insert(QStringLiteral("candidate"), QStringLiteral("qt"));
  ready.insert(
      QStringLiteral("classification"),
      QStringLiteral("functional-regression-with-observed-descendant-attribution"));
  ready.insert(QStringLiteral("state"), QStringLiteral("ready"));
  ready.insert(QStringLiteral("scenario_id"), arguments.scenario);
  ready.insert(QStringLiteral("tab_count"), arguments.tab_count);
  ready.insert(QStringLiteral("background_state"),
               arguments.background_state);
  ready.insert(QStringLiteral("layout"), arguments.layout);
  ready.insert(QStringLiteral("host_process_id"),
               static_cast<qint64>(QCoreApplication::applicationPid()));
  ready.insert(QStringLiteral("logical_content_bounds"),
               QJsonObject{{QStringLiteral("width"), kWidth},
                           {QStringLiteral("height"), kHeight}});
  ready.insert(QStringLiteral("observed_backing_scale"),
               window.devicePixelRatioF());
  ready.insert(QStringLiteral("startup_to_ready_ms"), clock.elapsed());
  ready.insert(QStringLiteral("profile_mode"),
               QStringLiteral("off-the-record"));
  ready.insert(QStringLiteral("headless_profiles"),
               QStringLiteral("isolated-off-the-record"));
  ready.insert(QStringLiteral("process_attribution"),
               QStringLiteral("observed_descendants_not_proven_complete"));
  ready.insert(QStringLiteral("visible"), records_json(visible));
  ready.insert(QStringLiteral("headless"), records_json(headless));
  write_atomic(QDir(arguments.evidence)
                   .filePath(QStringLiteral("memory-ready-v1.json")),
               ready);
  qInfo("QT_MEMORY ready");

  const QString stop_path =
      QDir(arguments.evidence).filePath(QStringLiteral("stop"));
  QElapsedTimer stop_clock;
  stop_clock.start();
  while (!QFileInfo::exists(stop_path) &&
         stop_clock.elapsed() < kDeadlineMilliseconds) {
    QApplication::processEvents(QEventLoop::AllEvents, 20);
    QThread::msleep(20);
  }
  if (!QFileInfo::exists(stop_path)) {
    throw std::runtime_error(
        "60-second stop deadline elapsed without stop file");
  }

  destroy_records(&visible);
  destroy_records(&headless);
  const bool is_collections_empty = visible.empty() && headless.empty();
  headless_first.reset();
  headless_second.reset();
  visible_profile.reset();
  window.close();
  if (!is_collections_empty) {
    throw std::runtime_error("owned record collections were not empty at shutdown");
  }
  QJsonObject shutdown;
  shutdown.insert(QStringLiteral("schema_version"), 1);
  shutdown.insert(QStringLiteral("candidate"), QStringLiteral("qt"));
  shutdown.insert(
      QStringLiteral("classification"),
      QStringLiteral("functional-regression-with-observed-descendant-attribution"));
  shutdown.insert(QStringLiteral("state"), QStringLiteral("shutdown"));
  shutdown.insert(QStringLiteral("scenario_id"), arguments.scenario);
  shutdown.insert(QStringLiteral("host_process_id"),
                  static_cast<qint64>(QCoreApplication::applicationPid()));
  shutdown.insert(QStringLiteral("is_owned_collections_empty"),
                  is_collections_empty);
  write_atomic(QDir(arguments.evidence)
                   .filePath(QStringLiteral("memory-shutdown-v1.json")),
               shutdown);
  qInfo("QT_MEMORY shutdown");
  return 0;
}
}
