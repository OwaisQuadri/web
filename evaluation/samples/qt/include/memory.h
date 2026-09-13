#pragma once

#include "rust/cxx.h"

namespace qt_smoke {

int run_memory_smoke(rust::Str fixture_origin, rust::Str evidence_directory,
                     rust::Str scenario_id, rust::Str tab_count,
                     rust::Str background_state, rust::Str layout);

}
