# Memory probe

This macOS probe reads process counters and checks a fixed browser workload. It reports per-process ledger values separately from the independently deduplicated `footprint` total.

## Commands

Use `--clock` immediately before launch and after readiness. It reports the shared macOS monotonic clock in nanoseconds and a Unix timestamp. The launch scripts use the monotonic pair for comparable external startup timing.

Use `--pid <owned-process-id>` for one process that the caller owns. The probe reports physical footprint, resident memory, lifetime peak, disk input and disk output. It also reports the raw process start identity.

Use `--validate-scenario <path>` to validate the fixed `memory-v1` scenario. The file must contain the approved 7, 12 and 20-tab cases, two headless sessions, fixed actions and fixed sample timing.

Use `--self-test <scratch-directory>` to check one disposable child. The child allocates private memory, maps a fixed read-only file and writes one new file. The command checks its identity and cumulative counters.

Use `--qualify-accounting <new-directory>` to check two owned children. Each child has a 16-mebibyte private allocation and the same one-mebibyte read-only mapping. Each child writes one mebibyte. The command requires the fixed memory and disk work, shared mapped-file evidence, and at least 64 kibibytes of aggregate sharing. It compares the non-deduplicated ledger with one `/usr/bin/footprint` aggregate and keeps both results.

Use `--sample-tree <owned-root-process-id> <new-directory> [correlated-process-id ...]` after a browser reports readiness. The command records 11 observations on a nominal one-second schedule. A slow `footprint` collection extends the window instead of dropping an observation. Extra process identifiers support launch-correlated macOS service processes. The result marks process membership incomplete because this command cannot prove ownership of those services.

Use `--validate-run <cef|webkit|qt> <scenario-id> <run-directory>` after clean shutdown. The `cef` value selects CEF(Chromium Embedded Framework). The command checks the scenario, page assignments, profile mode, native lifecycle evidence, external timing, process identities, and the complete sample window. It also requires matching input-checksum results, structured platform and dependency records, and an empty `build-processes-during.txt` receipt.

## Results

All result files use JSON(JavaScript Object Notation). Memory and disk counters use bytes. Elapsed values use nanoseconds unless their name ends in `_ms`. A start identity is a raw operating-system clock value, not a duration.

The sampler keeps raw `footprint` output for every observation. It records actual observation times, process churn, collection cost, machine pressure and the sampled deduplicated peak. Missing processes, reused identifiers, malformed output, warnings and errors fail the run. The sampler never adds lifetime peaks or calls a summed process ledger deduplicated.

## Native safety requirements

The probe uses libc 0.2.189 for process resource data and child enumeration. It allocates each native output buffer before the call. It keeps the buffer writable and alive until the call returns. It reads initialized data only after a successful return.

The qualification uses memmap2 0.9.11 for the shared file mapping. The parent creates the file before either child starts. No process changes the file until both mappings have dropped. The mapping remains alive while the child reads it.

The probe owns its qualification children and waits for both of them. Browser launch scripts own browser processes. They must verify process identity before cleanup and must keep unrelated applications open.
