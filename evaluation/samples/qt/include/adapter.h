#pragma once

#include "rust/cxx.h"

#include <memory>

namespace qt_smoke {

struct BrowserPolicy;
class QtSmokeAdapterImpl;

class QtSmokeAdapter {
public:
  explicit QtSmokeAdapter(rust::Box<BrowserPolicy> policy);
  ~QtSmokeAdapter();

  QtSmokeAdapter(const QtSmokeAdapter &) = delete;
  QtSmokeAdapter &operator=(const QtSmokeAdapter &) = delete;

  int run_smoke(rust::Str fixture_base_address, rust::Str evidence_directory,
                bool is_interactive, bool is_manual_smoke);

private:
  std::unique_ptr<QtSmokeAdapterImpl> impl_;
};

std::unique_ptr<QtSmokeAdapter> new_adapter(rust::Box<BrowserPolicy> policy);
int run_smoke(QtSmokeAdapter &adapter, rust::Str fixture_base_address,
              rust::Str evidence_directory, bool is_interactive,
              bool is_manual_smoke);

}
