mod policy;

use policy::{BrowserPolicy, SessionId, TabId};
use std::process::ExitCode;

#[cxx::bridge(namespace = "qt_smoke")]
mod ffi {
    struct PageTarget {
        session_id: u64,
        tab_id: u64,
        navigation_generation: u64,
    }

    extern "Rust" {
        type BrowserPolicy;

        fn new_browser_policy(session_id: u64, tab_id: u64) -> Box<BrowserPolicy>;
        fn is_tab_selected(policy: &mut BrowserPolicy, tab_id: u64) -> bool;
        fn is_selected_tab_granted(policy: &mut BrowserPolicy) -> bool;
        fn is_tab_granted(policy: &BrowserPolicy, tab_id: u64) -> bool;
        fn revoke_tab(policy: &mut BrowserPolicy, tab_id: u64);
        fn is_navigation_started(policy: &mut BrowserPolicy, tab_id: u64) -> bool;
        fn is_discarded(policy: &mut BrowserPolicy, tab_id: u64) -> bool;
        fn is_reload_started(policy: &mut BrowserPolicy, tab_id: u64) -> bool;
        fn is_tab_closed(policy: &mut BrowserPolicy, tab_id: u64) -> bool;
        fn end_session(policy: &mut BrowserPolicy);
        fn target_for(policy: &BrowserPolicy, tab_id: u64) -> PageTarget;
        fn is_control_allowed(policy: &BrowserPolicy, target: &PageTarget) -> bool;
    }

    unsafe extern "C++" {
        include!("adapter.h");
        include!("memory.h");

        type QtSmokeAdapter;

        fn new_adapter(policy: Box<BrowserPolicy>) -> Result<UniquePtr<QtSmokeAdapter>>;
        fn run_smoke(
            adapter: Pin<&mut QtSmokeAdapter>,
            fixture_base_address: &str,
            evidence_directory: &str,
            is_interactive: bool,
            is_manual_smoke: bool,
        ) -> Result<i32>;
        fn run_memory_smoke(
            fixture_origin: &str,
            evidence_directory: &str,
            scenario_id: &str,
            tab_count: &str,
            background_state: &str,
            layout: &str,
        ) -> Result<i32>;
    }
}

fn new_browser_policy(session_id: u64, tab_id: u64) -> Box<BrowserPolicy> {
    Box::new(BrowserPolicy::new(SessionId(session_id), [TabId(tab_id)]))
}

fn is_tab_selected(policy: &mut BrowserPolicy, tab_id: u64) -> bool {
    policy.is_tab_selected(TabId(tab_id))
}

fn is_selected_tab_granted(policy: &mut BrowserPolicy) -> bool {
    policy.is_selected_tab_granted()
}

fn is_tab_granted(policy: &BrowserPolicy, tab_id: u64) -> bool {
    policy.is_tab_granted(TabId(tab_id))
}

fn revoke_tab(policy: &mut BrowserPolicy, tab_id: u64) {
    policy.revoke(TabId(tab_id));
}

fn is_navigation_started(policy: &mut BrowserPolicy, tab_id: u64) -> bool {
    policy.is_navigation_started(TabId(tab_id))
}

fn is_discarded(policy: &mut BrowserPolicy, tab_id: u64) -> bool {
    policy.is_discarded(TabId(tab_id))
}

fn is_reload_started(policy: &mut BrowserPolicy, tab_id: u64) -> bool {
    policy.is_reload_started(TabId(tab_id))
}

fn is_tab_closed(policy: &mut BrowserPolicy, tab_id: u64) -> bool {
    policy.is_tab_closed(TabId(tab_id))
}

fn end_session(policy: &mut BrowserPolicy) {
    policy.end_session();
}

fn target_for(policy: &BrowserPolicy, tab_id: u64) -> ffi::PageTarget {
    policy.target_for(TabId(tab_id)).unwrap_or(ffi::PageTarget {
        session_id: 0,
        tab_id: 0,
        navigation_generation: u64::MAX,
    })
}

fn is_control_allowed(policy: &BrowserPolicy, target: &ffi::PageTarget) -> bool {
    policy.is_control_allowed(target)
}

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(memory) = MemoryArguments::parse(&arguments) {
        return match ffi::run_memory_smoke(
            &memory.fixture_origin,
            &memory.evidence_directory,
            &memory.scenario_id,
            &memory.tab_count,
            &memory.background_state,
            &memory.layout,
        ) {
            Ok(exit_code) => ExitCode::from(exit_code.clamp(0, 255) as u8),
            Err(error) => {
                eprintln!("qt-smoke: {error}");
                ExitCode::from(2)
            }
        };
    }
    let is_manual_smoke = matches!(arguments.first(), Some(mode) if mode == "--manual-smoke");
    let is_interactive =
        matches!(arguments.first(), Some(mode) if mode == "--manual" || mode == "--manual-smoke");
    let Some((fixture_base_address, evidence_directory)) = parse_arguments(&arguments) else {
        eprintln!(
            "usage: qt-smoke (--smoke|--manual|--manual-smoke) <fixture-base-address> <evidence-directory> | --memory-smoke <fixture-origin> <evidence-directory> <scenario-id> <tab-count> <background-state> <layout>"
        );
        return ExitCode::from(2);
    };

    let policy = Box::new(BrowserPolicy::new(SessionId(1), [TabId(1), TabId(2)]));
    let mut adapter = match ffi::new_adapter(policy) {
        Ok(adapter) => adapter,
        Err(error) => {
            eprintln!("qt-smoke: {error}");
            return ExitCode::from(2);
        }
    };
    let exit_code = match ffi::run_smoke(
        adapter.pin_mut(),
        fixture_base_address,
        evidence_directory,
        is_interactive,
        is_manual_smoke,
    ) {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("qt-smoke: {error}");
            2
        }
    };
    drop(adapter);
    ExitCode::from(exit_code.clamp(0, 255) as u8)
}

struct MemoryArguments {
    fixture_origin: String,
    evidence_directory: String,
    scenario_id: String,
    tab_count: String,
    background_state: String,
    layout: String,
}

impl MemoryArguments {
    fn parse(arguments: &[String]) -> Option<Self> {
        let [
            mode,
            fixture_origin,
            evidence_directory,
            scenario_id,
            tab_count,
            background_state,
            layout,
        ] = arguments
        else {
            return None;
        };
        let tab_count_value = tab_count.parse::<usize>().ok()?;
        let is_valid = matches!(
            (background_state.as_str(), layout.as_str(), tab_count_value),
            ("loaded", "single", 7 | 12 | 20) | ("unloaded", "single", 20) | ("loaded", "split", 7)
        );
        let expected_scenario = format!("{background_state}-{layout}-{tab_count}");
        (mode == "--memory-smoke" && is_valid && scenario_id == &expected_scenario).then(|| Self {
            fixture_origin: fixture_origin.clone(),
            evidence_directory: evidence_directory.clone(),
            scenario_id: scenario_id.clone(),
            tab_count: tab_count.clone(),
            background_state: background_state.clone(),
            layout: layout.clone(),
        })
    }
}

fn parse_arguments(arguments: &[String]) -> Option<(&str, &str)> {
    match arguments {
        [mode, fixture_base_address, evidence_directory]
            if mode == "--smoke" || mode == "--manual" || mode == "--manual-smoke" =>
        {
            Some((fixture_base_address, evidence_directory))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryArguments;

    fn arguments(scenario: &str, count: &str, background: &str, layout: &str) -> Vec<String> {
        [
            "--memory-smoke",
            "http://localhost:1",
            "evidence",
            scenario,
            count,
            background,
            layout,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn accepts_only_the_pinned_memory_scenarios() {
        for values in [
            ("loaded-single-7", "7", "loaded", "single"),
            ("loaded-single-12", "12", "loaded", "single"),
            ("loaded-single-20", "20", "loaded", "single"),
            ("unloaded-single-20", "20", "unloaded", "single"),
            ("loaded-split-7", "7", "loaded", "split"),
        ] {
            assert!(
                MemoryArguments::parse(&arguments(values.0, values.1, values.2, values.3))
                    .is_some()
            );
        }
        assert!(MemoryArguments::parse(&arguments("wrong", "7", "loaded", "single")).is_none());
        assert!(
            MemoryArguments::parse(&arguments("unloaded-split-7", "7", "unloaded", "split"))
                .is_none()
        );
    }
}
