use std::env;
use std::path::PathBuf;

fn main() {
    let qt_prefix = env::var_os("QT_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("QT_PREFIX must name the Qt 6.11.2 installation prefix");
        });
    let qt_frameworks = qt_prefix.join("lib");
    if !qt_frameworks.join("QtWebEngineWidgets.framework").is_dir() {
        panic!("QT_PREFIX does not contain QtWebEngineWidgets.framework");
    }

    let framework_search = format!("-F{}", qt_frameworks.display());
    cxx_build::bridge("src/main.rs")
        .file("src/adapter.cc")
        .file("src/memory.cc")
        .include("include")
        .include(qt_prefix.join("include"))
        .flag(&framework_search)
        .flag("-std=c++17")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-Werror")
        .warnings_into_errors(true)
        .compile("qt-smoke-native");

    for framework in [
        "QtWebEngineWidgets",
        "QtWebEngineCore",
        "QtWidgets",
        "QtGui",
        "QtCore",
        "QtNetwork",
        "QtPrintSupport",
        "QtQuick",
        "QtQuickWidgets",
        "QtQml",
        "QtWebChannel",
        "QtPositioning",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!(
        "cargo:rustc-link-search=framework={}",
        qt_frameworks.display()
    );
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        qt_frameworks.display()
    );
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=src/adapter.cc");
    println!("cargo:rerun-if-changed=src/memory.cc");
    println!("cargo:rerun-if-changed=include/adapter.h");
    println!("cargo:rerun-if-changed=include/memory.h");
    println!("cargo:rerun-if-env-changed=QT_PREFIX");
}
