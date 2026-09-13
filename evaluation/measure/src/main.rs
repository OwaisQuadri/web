use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MEBIBYTE: usize = 1024 * 1024;
const PRIVATE_ALLOCATION_BYTES: usize = 16 * MEBIBYTE;
const SHARED_FILE_BYTES: usize = MEBIBYTE;

#[derive(Debug, Clone, Serialize)]
struct Sample {
    pid: i32,
    start: u64,
    footprint: u64,
    resident: u64,
    lifetime_peak: u64,
    disk_read: u64,
    disk_write: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct ProcessIdentity {
    pid: i32,
    start: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct MembershipDelta {
    added: Vec<ProcessIdentity>,
    missing: Vec<ProcessIdentity>,
    reused: Vec<(ProcessIdentity, ProcessIdentity)>,
}

fn compare_membership(before: &[ProcessIdentity], after: &[ProcessIdentity]) -> MembershipDelta {
    let before_by_pid: BTreeMap<_, _> = before.iter().map(|item| (item.pid, *item)).collect();
    let after_by_pid: BTreeMap<_, _> = after.iter().map(|item| (item.pid, *item)).collect();
    let added = after_by_pid
        .iter()
        .filter(|(pid, _)| !before_by_pid.contains_key(pid))
        .map(|(_, identity)| *identity)
        .collect();
    let missing = before_by_pid
        .iter()
        .filter(|(pid, _)| !after_by_pid.contains_key(pid))
        .map(|(_, identity)| *identity)
        .collect();
    let reused = before_by_pid
        .iter()
        .filter_map(|(pid, before_identity)| {
            after_by_pid
                .get(pid)
                .filter(|after_identity| after_identity.start != before_identity.start)
                .map(|after_identity| (*before_identity, *after_identity))
        })
        .collect();
    MembershipDelta {
        added,
        missing,
        reused,
    }
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioDocument {
    schema_version: u64,
    fixture_revision: String,
    viewport: Viewport,
    headless_pages: Vec<String>,
    profile_mode: String,
    actions: Vec<String>,
    settle_ms: u64,
    sample_interval_ms: u64,
    sample_duration_ms: u64,
    repetitions: u64,
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Viewport {
    logical_width: u64,
    logical_height: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    id: String,
    visible_pages: Vec<String>,
    background_state: String,
    layout: String,
    is_recording: bool,
}

#[derive(Debug, Deserialize)]
struct FootprintOutput {
    unit: String,
    #[serde(rename = "bytes per unit")]
    bytes_per_unit: u64,
    #[serde(rename = "total footprint")]
    total_footprint: u64,
    processes: Vec<FootprintProcess>,
    errors: Vec<serde_json::Value>,
    warnings: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct FootprintProcess {
    pid: i32,
    name: String,
    footprint: u64,
}

#[derive(Serialize)]
struct QualificationResult {
    result_version: u64,
    outcome: String,
    ledger: Ledger,
    footprint: QualificationFootprint,
    children: Vec<ChildResult>,
}

#[derive(Serialize)]
struct Ledger {
    is_deduplicated: bool,
    sampled_physical_footprint_sum_bytes: u64,
    fixed_private_allocation_bytes_per_child: usize,
    fixed_shared_mapping_bytes: usize,
    known_disk_write_bytes_per_child: usize,
}

#[derive(Serialize)]
struct QualificationFootprint {
    is_independently_deduplicated: bool,
    total_bytes: u64,
    observed_aggregate_savings_bytes: u64,
    shared_mapped_clean_bytes: u64,
    collection_elapsed_ns: u128,
    raw_output_path: String,
}

#[derive(Serialize)]
struct ChildResult {
    pid: i32,
    start_abstime: u64,
    disk_write_before: u64,
    disk_write_after: u64,
}

#[derive(Serialize)]
struct TreeWindowResult {
    result_version: u64,
    outcome: String,
    scope: String,
    is_membership_complete: bool,
    correlated_extra_pids: Vec<i32>,
    root: ProcessIdentity,
    interval_ms: u64,
    duration_ms: u64,
    sampled_peak_deduplicated_bytes: u64,
    samples: Vec<TreeSample>,
}

#[derive(Serialize)]
struct TreeSample {
    index: usize,
    elapsed_ns: u128,
    footprint_collection_elapsed_ns: u128,
    is_ledger_deduplicated: bool,
    ledger_physical_footprint_sum_bytes: u64,
    independently_deduplicated_bytes: u64,
    members: Vec<Sample>,
    churn: MembershipDelta,
    raw_footprint_path: String,
}

fn parse_pid(value: &str) -> io::Result<i32> {
    let pid = value.parse::<i32>().map_err(io::Error::other)?;
    if pid <= 0 {
        return Err(io::Error::other("process id must be positive"));
    }
    Ok(pid)
}

fn validate_extra_pids(root_pid: i32, extra_pids: &[i32]) -> io::Result<()> {
    let unique: BTreeSet<_> = extra_pids.iter().copied().collect();
    if unique.len() != extra_pids.len() || unique.contains(&root_pid) {
        return Err(io::Error::other(
            "extra process ids must be unique and exclude the root",
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn sample(pid: i32) -> io::Result<Sample> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    let result = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            usage.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let usage = unsafe { usage.assume_init() };
    Ok(Sample {
        pid,
        start: usage.ri_proc_start_abstime,
        footprint: usage.ri_phys_footprint,
        resident: usage.ri_resident_size,
        lifetime_peak: usage.ri_lifetime_max_phys_footprint,
        disk_read: usage.ri_diskio_bytesread,
        disk_write: usage.ri_diskio_byteswritten,
    })
}

#[cfg(not(target_os = "macos"))]
fn sample(_pid: i32) -> io::Result<Sample> {
    Err(io::Error::other("this accounting probe requires macOS"))
}

fn direct_child_pids(parent_pid: i32) -> io::Result<Vec<i32>> {
    #[cfg(target_os = "macos")]
    {
        let mut capacity = 16_usize;
        loop {
            let mut pids = vec![0_i32; capacity];
            let bytes =
                i32::try_from(std::mem::size_of_val(pids.as_slice())).map_err(io::Error::other)?;
            let count =
                unsafe { libc::proc_listchildpids(parent_pid, pids.as_mut_ptr().cast(), bytes) };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            let count = usize::try_from(count).map_err(io::Error::other)?;
            if count < capacity {
                pids.truncate(count);
                return Ok(pids);
            }
            capacity = capacity
                .checked_mul(2)
                .ok_or_else(|| io::Error::other("child list too large"))?;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = parent_pid;
        Err(io::Error::other("direct child enumeration requires macOS"))
    }
}

fn descendant_pids_with(
    root_pid: i32,
    mut children_for: impl FnMut(i32) -> io::Result<Vec<i32>>,
) -> io::Result<Vec<i32>> {
    let mut seen = BTreeSet::from([root_pid]);
    let mut pending = VecDeque::from([root_pid]);
    while let Some(parent_pid) = pending.pop_front() {
        for child_pid in children_for(parent_pid)? {
            if child_pid <= 0 {
                return Err(io::Error::other("child process id must be positive"));
            }
            if seen.insert(child_pid) {
                pending.push_back(child_pid);
            }
        }
    }
    seen.remove(&root_pid);
    Ok(seen.into_iter().collect())
}

fn descendant_identities(root_pid: i32) -> io::Result<Vec<ProcessIdentity>> {
    descendant_pids_with(root_pid, direct_child_pids)?
        .into_iter()
        .map(|pid| {
            let value = sample(pid)?;
            Ok(ProcessIdentity {
                pid: value.pid,
                start: value.start,
            })
        })
        .collect()
}

fn emit(value: &Sample, elapsed_ns: u128) {
    println!(
        "{{\"kind\":\"process_sample\",\"scope\":\"single_process_not_deduplicated_total\",\"pid\":{},\"start_abstime\":{},\"elapsed_ns\":{},\"footprint_bytes\":{},\"resident_bytes\":{},\"lifetime_peak_bytes\":{},\"disk_read_bytes\":{},\"disk_write_bytes\":{}}}",
        value.pid,
        value.start,
        elapsed_ns,
        value.footprint,
        value.resident,
        value.lifetime_peak,
        value.disk_read,
        value.disk_write,
    );
}

fn verify_pair(before: &Sample, after: &Sample) -> io::Result<()> {
    if before.pid != after.pid || before.start == 0 || before.start != after.start {
        return Err(io::Error::other("process identity changed"));
    }
    if after.disk_read < before.disk_read || after.disk_write < before.disk_write {
        return Err(io::Error::other("cumulative disk counters decreased"));
    }
    Ok(())
}

fn validate_scenario(document: ScenarioDocument) -> io::Result<()> {
    if document.schema_version != 1
        || document.fixture_revision != "memory-v1"
        || document.viewport.logical_width != 800
        || document.viewport.logical_height != 600
        || document.headless_pages != ["article", "application"]
        || document.profile_mode != "off-the-record"
        || document.actions
            != [
                "select-each-visible-tab",
                "run-page-action",
                "select-first-visible-tab",
            ]
        || document.settle_ms != 5000
        || document.sample_interval_ms != 1000
        || document.sample_duration_ms != 10000
        || document.repetitions != 3
    {
        return Err(io::Error::other(
            "scenario constants do not match memory-v1",
        ));
    }
    let expected = [
        ("loaded-single-7", "loaded", "single", 7_usize),
        ("loaded-single-12", "loaded", "single", 12),
        ("loaded-single-20", "loaded", "single", 20),
        ("unloaded-single-20", "unloaded", "single", 20),
        ("loaded-split-7", "loaded", "split", 7),
    ];
    let assigned_pages = ["article", "images", "application"];
    if document.scenarios.len() != expected.len() {
        return Err(io::Error::other("scenario count does not match memory-v1"));
    }
    let mut ids = BTreeSet::new();
    for scenario in &document.scenarios {
        if !ids.insert(&scenario.id)
            || !["article", "images", "application"]
                .contains(&scenario.visible_pages.first().map_or("", String::as_str))
            || scenario
                .visible_pages
                .iter()
                .any(|page| !["article", "images", "application"].contains(&page.as_str()))
            || scenario.is_recording
        {
            return Err(io::Error::other(
                "scenario contains invalid identifiers or recording",
            ));
        }
        let Some((_, state, layout, tabs)) = expected.iter().find(|(id, ..)| *id == scenario.id)
        else {
            return Err(io::Error::other("unexpected scenario id"));
        };
        if scenario.background_state != *state
            || scenario.layout != *layout
            || scenario.visible_pages.len() != *tabs
            || scenario
                .visible_pages
                .iter()
                .map(String::as_str)
                .ne(assigned_pages.iter().copied().cycle().take(*tabs))
        {
            return Err(io::Error::other("scenario shape does not match memory-v1"));
        }
    }
    Ok(())
}

fn parse_footprint(bytes: &[u8], expected_pids: &[i32]) -> io::Result<FootprintOutput> {
    let output: FootprintOutput = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    if output.unit != "byte"
        || output.bytes_per_unit != 1
        || output.total_footprint == 0
        || !output.errors.is_empty()
        || !output.warnings.is_empty()
    {
        return Err(io::Error::other(
            "footprint output has invalid accounting metadata",
        ));
    }
    let expected: BTreeSet<_> = expected_pids.iter().copied().collect();
    let actual: BTreeSet<_> = output.processes.iter().map(|process| process.pid).collect();
    if expected.len() != expected_pids.len()
        || actual.len() != output.processes.len()
        || actual != expected
    {
        return Err(io::Error::other(
            "footprint process ids do not match requested set",
        ));
    }
    for process in &output.processes {
        if process.name.is_empty() || process.footprint == 0 {
            return Err(io::Error::other("footprint process has invalid fields"));
        }
    }
    if output
        .processes
        .iter()
        .any(|process| process.footprint > output.total_footprint)
    {
        return Err(io::Error::other("process footprint exceeds total"));
    }
    Ok(output)
}

fn shared_mapped_clean_bytes(bytes: &[u8]) -> io::Result<u64> {
    let document: serde_json::Value = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    document["shared"]
        .as_array()
        .and_then(|groups| {
            groups
                .iter()
                .filter_map(|group| group["categories"]["mapped file"]["clean"].as_u64())
                .max()
        })
        .ok_or_else(|| io::Error::other("footprint output has no shared mapped-file evidence"))
}

fn expect_line(reader: &mut impl BufRead, expected: &str) -> io::Result<()> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim_end() != expected {
        return Err(io::Error::other(format!(
            "expected {expected:?}, got {line:?}"
        )));
    }
    Ok(())
}

fn fixture(directory: &Path, shared_path: &Path) -> io::Result<()> {
    let private_memory = vec![0x5a_u8; PRIVATE_ALLOCATION_BYTES];
    let shared_file = File::open(shared_path)?;
    // The parent creates the file before launch and never mutates it while either child runs.
    let shared_mapping = unsafe { Mmap::map(&shared_file)? };
    for offset in (0..shared_mapping.len()).step_by(4096) {
        std::hint::black_box(shared_mapping[offset]);
    }
    std::hint::black_box((&private_memory, &shared_mapping));
    println!("ready");
    io::stdout().flush()?;
    let mut signal = [0_u8; 1];
    io::stdin().read_exact(&mut signal)?;
    if signal != [b'w'] {
        return Err(io::Error::other("invalid fixture command"));
    }
    let path = directory.join(format!("probe-fixture-{}.bin", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&private_memory[..MEBIBYTE])?;
    file.sync_all()?;
    println!("written");
    io::stdout().flush()?;
    let _ = io::stdin().read(&mut signal)?;
    std::hint::black_box((private_memory, shared_mapping));
    Ok(())
}

fn spawn_fixture(
    directory: &Path,
    shared_path: &Path,
) -> io::Result<(OwnedChild, BufReader<impl Read>, impl Write)> {
    let mut child = OwnedChild(
        Command::new(std::env::current_exe()?)
            .arg("--fixture-child")
            .arg(directory)
            .arg(shared_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?,
    );
    let reader = BufReader::new(
        child
            .0
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing output"))?,
    );
    let writer = child
        .0
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing input"))?;
    Ok((child, reader, writer))
}

fn self_test(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let shared = directory.join("self-test-shared.bin");
    fs::write(&shared, vec![0_u8; SHARED_FILE_BYTES])?;
    let (mut child, mut reader, mut writer) = spawn_fixture(directory, &shared)?;
    expect_line(&mut reader, "ready")?;
    let pid = i32::try_from(child.0.id()).map_err(io::Error::other)?;
    let start = Instant::now();
    let before = sample(pid)?;
    emit(&before, start.elapsed().as_nanos());
    writer.write_all(b"w")?;
    writer.flush()?;
    expect_line(&mut reader, "written")?;
    let after = sample(pid)?;
    emit(&after, start.elapsed().as_nanos());
    verify_pair(&before, &after)?;
    if before.footprint == 0 || before.start == 0 {
        return Err(io::Error::other("native counters unavailable"));
    }
    drop(writer);
    if !child.0.wait()?.success() {
        return Err(io::Error::other("fixture child failed"));
    }
    println!(
        "{{\"kind\":\"self_test\",\"outcome\":\"passed\",\"scope\":\"counter_read_and_identity_only\"}}"
    );
    Ok(())
}

fn verify_owned_children(expected: &[ProcessIdentity]) -> io::Result<()> {
    let root_pid = i32::try_from(std::process::id()).map_err(io::Error::other)?;
    let actual = descendant_identities(root_pid)?;
    let delta = compare_membership(expected, &actual);
    if !delta.added.is_empty() || !delta.missing.is_empty() || !delta.reused.is_empty() {
        return Err(io::Error::other("owned child membership changed"));
    }
    Ok(())
}

fn qualify_accounting(directory: &Path) -> io::Result<()> {
    fs::create_dir(directory)?;
    let shared = directory.join("shared-read-only.bin");
    fs::write(&shared, vec![0x31_u8; SHARED_FILE_BYTES])?;
    let (mut first, mut first_reader, mut first_writer) = spawn_fixture(directory, &shared)?;
    expect_line(&mut first_reader, "ready")?;
    let (mut second, mut second_reader, second_writer) = spawn_fixture(directory, &shared)?;
    expect_line(&mut second_reader, "ready")?;
    let pids = [
        i32::try_from(first.0.id()).map_err(io::Error::other)?,
        i32::try_from(second.0.id()).map_err(io::Error::other)?,
    ];
    let before = [sample(pids[0])?, sample(pids[1])?];
    let identities: Vec<_> = before
        .iter()
        .map(|value| ProcessIdentity {
            pid: value.pid,
            start: value.start,
        })
        .collect();
    verify_owned_children(&identities)?;
    first_writer.write_all(b"w")?;
    first_writer.flush()?;
    expect_line(&mut first_reader, "written")?;
    let mut second_writer = second_writer;
    second_writer.write_all(b"w")?;
    second_writer.flush()?;
    expect_line(&mut second_reader, "written")?;
    let after = [sample(pids[0])?, sample(pids[1])?];
    for (before, after) in before.iter().zip(&after) {
        verify_pair(before, after)?;
    }
    verify_owned_children(&identities)?;
    let raw_path = directory.join("footprint-raw.json");
    let footprint_started = Instant::now();
    let status = Command::new("/usr/bin/footprint")
        .arg("-j")
        .arg(&raw_path)
        .arg("--pid")
        .arg(pids[0].to_string())
        .arg("--pid")
        .arg(pids[1].to_string())
        .status()?;
    let footprint_elapsed_ns = footprint_started.elapsed().as_nanos();
    if !status.success() {
        return Err(io::Error::other("footprint command failed"));
    }
    let footprint_bytes = fs::read(&raw_path)?;
    let footprint = parse_footprint(&footprint_bytes, &pids)?;
    let shared_mapped_clean_bytes = shared_mapped_clean_bytes(&footprint_bytes)?;
    verify_owned_children(&identities)?;
    drop(first_writer);
    drop(second_writer);
    if !first.0.wait()?.success() || !second.0.wait()?.success() {
        return Err(io::Error::other("fixture child failed"));
    }
    if identities
        .iter()
        .any(|identity| sample(identity.pid).is_ok())
    {
        return Err(io::Error::other("fixture child survived cleanup"));
    }
    let sampled_physical_footprint_sum_bytes = after.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(value.footprint)
            .ok_or_else(|| io::Error::other("ledger footprint sum overflowed"))
    })?;
    for (before, after) in before.iter().zip(&after) {
        if after
            .disk_write
            .checked_sub(before.disk_write)
            .is_none_or(|bytes| bytes < MEBIBYTE as u64)
            || after.footprint < PRIVATE_ALLOCATION_BYTES as u64
        {
            return Err(io::Error::other("qualification counters missed fixed work"));
        }
    }
    if footprint.total_footprint == 0
        || footprint.total_footprint > sampled_physical_footprint_sum_bytes
        || sampled_physical_footprint_sum_bytes - footprint.total_footprint < 64 * 1024
        || shared_mapped_clean_bytes < SHARED_FILE_BYTES as u64
    {
        return Err(io::Error::other(
            "footprint aggregate did not demonstrate shared-memory deduplication",
        ));
    }
    let result = QualificationResult {
        result_version: 1,
        outcome: "qualified".to_owned(),
        ledger: Ledger {
            is_deduplicated: false,
            sampled_physical_footprint_sum_bytes,
            fixed_private_allocation_bytes_per_child: PRIVATE_ALLOCATION_BYTES,
            fixed_shared_mapping_bytes: SHARED_FILE_BYTES,
            known_disk_write_bytes_per_child: MEBIBYTE,
        },
        footprint: QualificationFootprint {
            is_independently_deduplicated: true,
            total_bytes: footprint.total_footprint,
            observed_aggregate_savings_bytes: sampled_physical_footprint_sum_bytes
                - footprint.total_footprint,
            shared_mapped_clean_bytes,
            collection_elapsed_ns: footprint_elapsed_ns,
            raw_output_path: "footprint-raw.json".to_owned(),
        },
        children: before
            .iter()
            .zip(&after)
            .map(|(before, after)| ChildResult {
                pid: before.pid,
                start_abstime: before.start,
                disk_write_before: before.disk_write,
                disk_write_after: after.disk_write,
            })
            .collect(),
    };
    let encoded = serde_json::to_vec_pretty(&result).map_err(io::Error::other)?;
    let temporary = directory.join("qualification-result-v1.json.partial");
    let final_path = directory.join("qualification-result-v1.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, final_path)?;
    println!("{}", String::from_utf8(encoded).map_err(io::Error::other)?);
    Ok(())
}

fn validate_scenario_file(path: &Path) -> io::Result<()> {
    let document = serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)?;
    validate_scenario(document)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::other("result already exists"));
    }
    let pending = path.with_extension("partial");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(pending, path)
}

fn save_command_output(path: &Path, output: std::process::Output) -> io::Result<bool> {
    let is_success = output.status.success();
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    atomic_write(path, &bytes)?;
    Ok(is_success)
}

fn write_command_output(path: &Path, command: &mut Command) -> io::Result<()> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait()?.is_some() {
            return if save_command_output(path, child.wait_with_output()?)? {
                Ok(())
            } else {
                Err(io::Error::other("system observation command failed"))
            };
        }
        if Instant::now() >= deadline {
            child.kill()?;
            save_command_output(path, child.wait_with_output()?)?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "system observation command exceeded 30 seconds",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn current_tree(root_pid: i32, extra_pids: &[i32]) -> io::Result<Vec<Sample>> {
    let mut pids = BTreeSet::from([root_pid]);
    pids.extend(descendant_pids_with(root_pid, direct_child_pids)?);
    pids.extend(extra_pids.iter().copied());
    pids.into_iter().map(sample).collect()
}

fn run_footprint(directory: &Path, index: usize, members: &[Sample]) -> io::Result<(u64, u128)> {
    let raw_name = format!("footprint-{index:02}.json");
    let raw_path = directory.join(&raw_name);
    let summary_path = directory.join(format!("footprint-{index:02}.txt"));
    let mut command = Command::new("/usr/bin/footprint");
    command.arg("-j").arg(&raw_path).arg("--noCategories");
    for member in members {
        command.arg("--pid").arg(member.pid.to_string());
    }
    let started = Instant::now();
    write_command_output(&summary_path, &mut command)?;
    let elapsed_ns = started.elapsed().as_nanos();
    let pids: Vec<_> = members.iter().map(|member| member.pid).collect();
    let output = parse_footprint(&fs::read(raw_path)?, &pids)?;
    Ok((output.total_footprint, elapsed_ns))
}

fn sample_tree(root_pid: i32, directory: &Path, extra_pids: &[i32]) -> io::Result<()> {
    const INTERVAL_MS: u64 = 1000;
    const DURATION_MS: u64 = 10000;
    const SAMPLE_COUNT: usize = 11;

    fs::create_dir(directory)?;
    write_command_output(
        &directory.join("memory-pressure-before.txt"),
        Command::new("/usr/bin/memory_pressure").arg("-Q"),
    )?;
    let initial_root = sample(root_pid)?;
    let root = ProcessIdentity {
        pid: initial_root.pid,
        start: initial_root.start,
    };
    let started = Instant::now();
    let mut previous_members = Vec::new();
    let mut samples = Vec::new();
    let mut next_sample_ms = 0_u64;
    loop {
        let members = current_tree(root_pid, extra_pids)?;
        let observed_root = members
            .iter()
            .find(|member| member.pid == root_pid)
            .ok_or_else(|| io::Error::other("root process disappeared"))?;
        if observed_root.start != root.start {
            return Err(io::Error::other("root process identity changed"));
        }
        let identities: Vec<_> = members
            .iter()
            .map(|member| ProcessIdentity {
                pid: member.pid,
                start: member.start,
            })
            .collect();
        let churn = if previous_members.is_empty() {
            MembershipDelta {
                added: Vec::new(),
                missing: Vec::new(),
                reused: Vec::new(),
            }
        } else {
            compare_membership(&previous_members, &identities)
        };
        if !churn.reused.is_empty() {
            return Err(io::Error::other("process identifier was reused"));
        }
        let index = samples.len();
        let elapsed_ns = started.elapsed().as_nanos();
        let (independently_deduplicated_bytes, footprint_collection_elapsed_ns) =
            run_footprint(directory, index, &members)?;
        let members_after_collection = current_tree(root_pid, extra_pids)?;
        let identities_after_collection = members_after_collection
            .iter()
            .map(|member| ProcessIdentity {
                pid: member.pid,
                start: member.start,
            })
            .collect::<Vec<_>>();
        let collection_delta = compare_membership(&identities, &identities_after_collection);
        if !collection_delta.added.is_empty()
            || !collection_delta.missing.is_empty()
            || !collection_delta.reused.is_empty()
        {
            return Err(io::Error::other(
                "process membership changed during footprint collection",
            ));
        }
        let ledger_physical_footprint_sum_bytes =
            members.iter().try_fold(0_u64, |sum, member| {
                sum.checked_add(member.footprint)
                    .ok_or_else(|| io::Error::other("ledger footprint sum overflowed"))
            })?;
        samples.push(TreeSample {
            index,
            elapsed_ns,
            footprint_collection_elapsed_ns,
            is_ledger_deduplicated: false,
            ledger_physical_footprint_sum_bytes,
            independently_deduplicated_bytes,
            members,
            churn,
            raw_footprint_path: format!("footprint-{index:02}.json"),
        });
        previous_members = identities;
        if samples.len() == SAMPLE_COUNT {
            break;
        }
        next_sample_ms = next_sample_ms
            .checked_add(INTERVAL_MS)
            .ok_or_else(|| io::Error::other("sample schedule overflowed"))?;
        let next_sample = Duration::from_millis(next_sample_ms);
        if let Some(remaining) = next_sample.checked_sub(started.elapsed()) {
            std::thread::sleep(remaining);
        }
    }
    write_command_output(
        &directory.join("memory-pressure-after.txt"),
        Command::new("/usr/bin/memory_pressure").arg("-Q"),
    )?;
    let sampled_peak_deduplicated_bytes = samples
        .iter()
        .map(|value| value.independently_deduplicated_bytes)
        .max()
        .ok_or_else(|| io::Error::other("sample window is empty"))?;
    let result = TreeWindowResult {
        result_version: 1,
        outcome: "complete".to_owned(),
        scope: if extra_pids.is_empty() {
            "root_and_observed_descendants_with_independent_deduplication"
        } else {
            "root_descendants_and_launch_correlated_extra_processes_with_independent_deduplication"
        }
        .to_owned(),
        is_membership_complete: false,
        correlated_extra_pids: extra_pids.to_vec(),
        root,
        interval_ms: INTERVAL_MS,
        duration_ms: DURATION_MS,
        sampled_peak_deduplicated_bytes,
        samples,
    };
    let bytes = serde_json::to_vec_pretty(&result).map_err(io::Error::other)?;
    atomic_write(&directory.join("tree-window-v1.json"), &bytes)
}

fn scenario_shape(id: &str) -> Option<(usize, &'static str, &'static str)> {
    match id {
        "loaded-single-7" => Some((7, "loaded", "single")),
        "loaded-single-12" => Some((12, "loaded", "single")),
        "loaded-single-20" => Some((20, "loaded", "single")),
        "unloaded-single-20" => Some((20, "unloaded", "single")),
        "loaded-split-7" => Some((7, "loaded", "split")),
        _ => None,
    }
}

fn is_route_matching(url: &str, expected: &str) -> bool {
    let Some((scheme, remainder)) = url.split_once("://") else {
        return false;
    };
    if scheme != "http" {
        return false;
    }
    let Some((authority, path)) = remainder.split_once('/') else {
        return false;
    };
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]") && path == format!("{expected}.html")
}

fn read_json(path: &Path) -> io::Result<serde_json::Value> {
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}

fn validate_no_build_activity(directory: &Path) -> io::Result<()> {
    if fs::read(directory.join("build-processes-during.txt"))?.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other("build activity contaminated the run"))
    }
}

fn checksum_paths(path: &Path) -> io::Result<Vec<String>> {
    let text = fs::read_to_string(path)?;
    let paths = text
        .lines()
        .map(|line| {
            let (digest, path) = line
                .split_once("  ")
                .ok_or_else(|| io::Error::other("checksum line has invalid fields"))?;
            if digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                || path.is_empty()
            {
                return Err(io::Error::other("checksum line has invalid fields"));
            }
            Ok(path.to_owned())
        })
        .collect::<io::Result<Vec<_>>>()?;
    if paths.is_empty() {
        return Err(io::Error::other("checksum file is empty"));
    }
    Ok(paths)
}

fn verify_checksum_manifest(path: &Path) -> io::Result<()> {
    let status = Command::new("/usr/bin/shasum")
        .args(["-a", "256", "-c"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("checksum verification failed"))
    }
}

fn validate_provenance(directory: &Path) -> io::Result<()> {
    let platform = fs::read_to_string(directory.join("platform-provenance.txt"))?;
    if !["ProductName:", "ProductVersion:", "BuildVersion:"]
        .into_iter()
        .all(|field| platform.contains(field))
    {
        return Err(io::Error::other("platform provenance is incomplete"));
    }

    let dependencies = directory.join("dependencies.sha256");
    checksum_paths(&dependencies)?;
    verify_checksum_manifest(&dependencies)?;
    let inputs = directory.join("inputs.sha256");
    let expected_paths = checksum_paths(&inputs)?;
    verify_checksum_manifest(&inputs)?;
    let checked_paths = fs::read_to_string(directory.join("inputs-after.txt"))?
        .lines()
        .map(|line| {
            line.strip_suffix(": OK")
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other("input checksum result is invalid"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    if checked_paths != expected_paths {
        return Err(io::Error::other(
            "input checksum results do not match inputs",
        ));
    }
    Ok(())
}

fn is_webkit_shutdown_complete(shutdown: &serde_json::Value, tab_count: usize) -> bool {
    shutdown["is_native_release_observed"] == true
        && [("visible", tab_count), ("headless", 2)]
            .into_iter()
            .all(|(field, expected_count)| {
                shutdown[field].as_array().is_some_and(|records| {
                    records.len() == expected_count
                        && records.iter().all(|record| {
                            record["is_owned_reference_released"] == true
                                && record["is_native_release_observed"] == true
                        })
                })
            })
}

fn validate_browser_run(candidate: &str, scenario_id: &str, directory: &Path) -> io::Result<()> {
    validate_no_build_activity(directory)?;
    let (tab_count, background_state, layout) =
        scenario_shape(scenario_id).ok_or_else(|| io::Error::other("unknown scenario"))?;
    let ready = read_json(&directory.join("memory-ready-v1.json"))?;
    let shutdown = read_json(&directory.join("memory-shutdown-v1.json"))?;
    let measurement = read_json(&directory.join("measurement/tree-window-v1.json"))?;
    let host_process_id = ready["host_process_id"]
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| io::Error::other("ready receipt has no host process identity"))?;
    if ready["schema_version"] != 1
        || ready["candidate"] != candidate
        || ready["state"] != "ready"
        || ready["scenario_id"] != scenario_id
        || ready["tab_count"] != tab_count
        || ready["background_state"] != background_state
        || ready["layout"] != layout
        || ready["profile_mode"] != "off-the-record"
        || shutdown["schema_version"] != 1
        || shutdown["candidate"] != candidate
        || shutdown["state"] != "shutdown"
        || shutdown["scenario_id"] != scenario_id
        || shutdown["host_process_id"] != host_process_id
    {
        return Err(io::Error::other("browser receipts do not match the run"));
    }
    let (visible, headless) = if candidate == "cef" {
        let browsers = ready["browsers"]
            .as_array()
            .ok_or_else(|| io::Error::other("CEF receipt has no browser records"))?;
        if browsers.len() != tab_count + 2 {
            return Err(io::Error::other("CEF browser count is wrong"));
        }
        (&browsers[..tab_count], &browsers[tab_count..])
    } else {
        let visible = ready["visible"]
            .as_array()
            .ok_or_else(|| io::Error::other("ready receipt has no visible records"))?;
        let headless = ready["headless"]
            .as_array()
            .ok_or_else(|| io::Error::other("ready receipt has no headless records"))?;
        (visible.as_slice(), headless.as_slice())
    };
    if visible.len() != tab_count || headless.len() != 2 {
        return Err(io::Error::other("browser record count is wrong"));
    }
    for (index, record) in visible.iter().enumerate() {
        let expected = ["article", "images", "application"][index % 3];
        let role = if index == 0 { "visible" } else { "background" };
        let is_expected_unloaded = background_state == "unloaded" && index > 0;
        let is_lifecycle_matching = match candidate {
            "cef" => record["closed"] == is_expected_unloaded,
            "webkit" => {
                record["is_owned_reference_released"] == is_expected_unloaded
                    && (!is_expected_unloaded || record["is_native_release_observed"] == true)
            }
            "qt" => {
                record["lifecycle_state"]
                    == if is_expected_unloaded {
                        "discarded"
                    } else {
                        "active"
                    }
            }
            _ => false,
        };
        if record["role"] != role
            || !record["url"]
                .as_str()
                .is_some_and(|url| is_route_matching(url, expected))
        {
            return Err(io::Error::other("visible browser assignment is wrong"));
        }
        if !is_lifecycle_matching {
            return Err(io::Error::other("visible browser lifecycle is wrong"));
        }
    }
    for (record, expected_role, expected_route) in [
        (&headless[0], "headless-a", "article"),
        (&headless[1], "headless-b", "application"),
    ] {
        let is_lifecycle_matching = match candidate {
            "cef" => record["closed"] == false,
            "webkit" => record["is_owned_reference_released"] == false,
            "qt" => record["lifecycle_state"] == "active",
            _ => false,
        };
        if record["role"] != expected_role
            || !is_lifecycle_matching
            || !record["url"]
                .as_str()
                .is_some_and(|url| is_route_matching(url, expected_route))
        {
            return Err(io::Error::other("headless browser assignment is wrong"));
        }
    }
    let is_shutdown_complete = match candidate {
        "cef" => shutdown["browsers"].as_array().is_some_and(|records| {
            records.len() == tab_count + 2 && records.iter().all(|record| record["closed"] == true)
        }),
        "webkit" => is_webkit_shutdown_complete(&shutdown, tab_count),
        "qt" => shutdown["is_owned_collections_empty"] == true,
        _ => false,
    };
    if !is_shutdown_complete {
        return Err(io::Error::other("browser shutdown receipt is incomplete"));
    }
    let samples = measurement["samples"]
        .as_array()
        .ok_or_else(|| io::Error::other("measurement has no samples"))?;
    let sampled_peak = measurement["sampled_peak_deduplicated_bytes"].as_u64();
    let calculated_peak = samples
        .iter()
        .filter_map(|sample| sample["independently_deduplicated_bytes"].as_u64())
        .max();
    if measurement["result_version"] != 1
        || measurement["outcome"] != "complete"
        || measurement["root"]["pid"] != host_process_id
        || measurement["is_membership_complete"] != false
        || (candidate == "webkit"
            && measurement["correlated_extra_pids"]
                .as_array()
                .is_none_or(Vec::is_empty))
        || samples.len() != 11
        || samples
            .last()
            .and_then(|sample| sample["elapsed_ns"].as_u64())
            < Some(10_000_000_000)
        || sampled_peak.is_none()
        || sampled_peak != calculated_peak
        || samples.iter().any(|sample| {
            let total = sample["independently_deduplicated_bytes"].as_u64();
            total.is_none_or(|value| value == 0)
                || sample["is_ledger_deduplicated"] != false
                || sample["members"].as_array().is_none_or(|members| {
                    members.is_empty()
                        || members.iter().any(|member| {
                            member["footprint"].as_u64().zip(total).is_none_or(
                                |(footprint, total)| footprint == 0 || footprint > total,
                            )
                        })
                })
                || ["added", "missing", "reused"].into_iter().any(|field| {
                    sample["churn"][field]
                        .as_array()
                        .is_none_or(|values| !values.is_empty())
                })
        })
    {
        return Err(io::Error::other("measurement window is incomplete"));
    }
    validate_provenance(directory)?;
    let timing = read_json(&directory.join("startup-timing-v1.json"))?;
    if timing["schema_version"] != 1
        || timing["external_startup_to_ready_ns"]
            .as_u64()
            .is_none_or(|value| value == 0)
        || timing["start_monotonic_time_ns"]
            .as_u64()
            .zip(timing["ready_monotonic_time_ns"].as_u64())
            .is_none_or(|(start, ready)| ready <= start)
        || timing["start_unix_time_ns"].as_u64().is_none()
        || timing["ready_unix_time_ns"].as_u64().is_none()
    {
        return Err(io::Error::other("external startup timing is invalid"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn monotonic_time_ns() -> io::Result<u128> {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::zeroed();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, value.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let value = unsafe { value.assume_init() };
    let seconds = u128::try_from(value.tv_sec).map_err(io::Error::other)?;
    let nanoseconds = u128::try_from(value.tv_nsec).map_err(io::Error::other)?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(nanoseconds))
        .ok_or_else(|| io::Error::other("monotonic clock overflowed"))
}

#[cfg(not(target_os = "macos"))]
fn monotonic_time_ns() -> io::Result<u128> {
    Err(io::Error::other("monotonic clock requires macOS"))
}

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [action] if action == "--clock" => {
            let unix_time_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let monotonic_time_ns = monotonic_time_ns()?;
            println!(
                "{{\"kind\":\"clock\",\"unix_time_ns\":{unix_time_ns},\"monotonic_time_ns\":{monotonic_time_ns}}}"
            );
            Ok(())
        }
        [action, value] if action == "--pid" => {
            let start = Instant::now();
            emit(&sample(parse_pid(value)?)?, start.elapsed().as_nanos());
            Ok(())
        }
        [action, directory] if action == "--self-test" => self_test(Path::new(directory)),
        [action, path] if action == "--validate-scenario" => {
            validate_scenario_file(Path::new(path))
        }
        [action, directory] if action == "--qualify-accounting" => {
            qualify_accounting(Path::new(directory))
        }
        [action, candidate, scenario_id, directory] if action == "--validate-run" => {
            if !matches!(candidate.as_str(), "cef" | "webkit" | "qt") {
                return Err(io::Error::other("unknown browser candidate"));
            }
            validate_browser_run(candidate, scenario_id, Path::new(directory))
        }
        [action, root_pid, directory, extra_pids @ ..] if action == "--sample-tree" => {
            let root_pid = parse_pid(root_pid)?;
            let extra_pids = extra_pids
                .iter()
                .map(|value| parse_pid(value))
                .collect::<io::Result<Vec<_>>>()?;
            validate_extra_pids(root_pid, &extra_pids)?;
            sample_tree(root_pid, Path::new(directory), &extra_pids)
        }
        [action, directory, shared_path] if action == "--fixture-child" => {
            fixture(Path::new(directory), Path::new(shared_path))
        }
        _ => Err(io::Error::other(
            "usage: web-memory-probe --clock | --pid <owned-process-id> | --self-test <scratch-directory> | --validate-scenario <path> | --qualify-accounting <scratch-directory> | --sample-tree <owned-root-process-id> <output-directory> [correlated-process-id ...] | --validate-run <cef|webkit|qt> <scenario-id> <run-directory>",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(pid: i32, start: u64, disk_write: u64) -> Sample {
        Sample {
            pid,
            start,
            footprint: 100,
            resident: 200,
            lifetime_peak: 150,
            disk_read: 0,
            disk_write,
        }
    }

    #[test]
    fn accepts_the_pinned_scenario() {
        validate_scenario_file(Path::new("../scenarios/memory-v1.json")).unwrap();
    }

    #[test]
    fn rejects_unknown_scenario_fields() {
        let document = br#"{"schema_version":1,"fixture_revision":"memory-v1","viewport":{"logical_width":800,"logical_height":600},"headless_pages":["article","application"],"profile_mode":"off-the-record","actions":["select-each-visible-tab","run-page-action","select-first-visible-tab"],"settle_ms":5000,"sample_interval_ms":1000,"sample_duration_ms":10000,"repetitions":3,"scenarios":[],"unexpected":true}"#;
        assert!(serde_json::from_slice::<ScenarioDocument>(document).is_err());
    }

    #[test]
    fn parses_sanitized_two_process_footprint_fixture() {
        let bytes = include_bytes!("../tests/fixtures/footprint-two-process.json");
        let output = parse_footprint(bytes, &[101, 102]).unwrap();
        assert_eq!(output.total_footprint, 36_094_672);
        assert_eq!(shared_mapped_clean_bytes(bytes).unwrap(), 1_048_576);
    }

    #[test]
    fn rejects_process_larger_than_aggregate_total() {
        let single = br#"{"unit":"byte","bytes per unit":1,"total footprint":1,"processes":[{"pid":101,"name":"fixture","footprint":2}],"errors":[],"warnings":[]}"#;
        assert!(parse_footprint(single, &[101]).is_err());

        let multiple = br#"{"unit":"byte","bytes per unit":1,"total footprint":1,"processes":[{"pid":101,"name":"first","footprint":2},{"pid":102,"name":"second","footprint":3}],"errors":[],"warnings":[]}"#;
        assert!(parse_footprint(multiple, &[101, 102]).is_err());
    }

    #[test]
    fn requires_an_empty_build_activity_receipt() {
        let directory =
            std::env::temp_dir().join(format!("web-memory-build-receipt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        assert!(validate_no_build_activity(&directory).is_err());

        let receipt = directory.join("build-processes-during.txt");
        fs::write(&receipt, []).unwrap();
        validate_no_build_activity(&directory).unwrap();
        fs::write(&receipt, "cargo\n").unwrap();
        assert!(validate_no_build_activity(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn provenance_requires_matching_checksum_results() {
        let directory =
            std::env::temp_dir().join(format!("web-memory-provenance-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("platform-provenance.txt"),
            "ProductName:\tmacOS\nProductVersion:\t27.0\nBuildVersion:\t1\n",
        )
        .unwrap();
        let dependency = directory.join("dependency");
        let input = directory.join("input");
        fs::write(&dependency, []).unwrap();
        fs::write(&input, []).unwrap();
        let empty_digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        fs::write(
            directory.join("dependencies.sha256"),
            format!("{empty_digest}  {}\n", dependency.display()),
        )
        .unwrap();
        fs::write(
            directory.join("inputs.sha256"),
            format!("{empty_digest}  {}\n", input.display()),
        )
        .unwrap();
        fs::write(
            directory.join("inputs-after.txt"),
            format!("{}: OK\n", input.display()),
        )
        .unwrap();
        validate_provenance(&directory).unwrap();

        fs::write(
            directory.join("dependencies.sha256"),
            format!("{empty_digest}  missing\n"),
        )
        .unwrap();
        assert!(validate_provenance(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn webkit_shutdown_requires_exact_released_record_counts() {
        let released = serde_json::json!({
            "is_owned_reference_released": true,
            "is_native_release_observed": true,
        });
        let complete = serde_json::json!({
            "is_native_release_observed": true,
            "visible": [released.clone()],
            "headless": [released.clone(), released],
        });
        assert!(is_webkit_shutdown_complete(&complete, 1));

        let empty = serde_json::json!({
            "is_native_release_observed": true,
            "visible": [],
            "headless": [],
        });
        assert!(!is_webkit_shutdown_complete(&empty, 1));
    }

    #[test]
    fn accepts_positive_process_ids() {
        assert_eq!(parse_pid("42").unwrap(), 42);
        assert!(validate_extra_pids(42, &[43, 44]).is_ok());
        assert!(validate_extra_pids(42, &[42]).is_err());
        assert!(validate_extra_pids(42, &[43, 43]).is_err());
    }

    #[test]
    fn validates_scenario_shapes_and_local_routes() {
        assert_eq!(
            scenario_shape("loaded-single-12"),
            Some((12, "loaded", "single"))
        );
        assert!(scenario_shape("loaded-split-12").is_none());
        assert!(is_route_matching(
            "http://localhost:42/article.html",
            "article"
        ));
        assert!(!is_route_matching(
            "https://localhost:42/article.html",
            "article"
        ));
        assert!(!is_route_matching(
            "http://example.test:42/article.html",
            "article"
        ));
        assert!(!is_route_matching(
            "http://localhost:42/images.html",
            "article"
        ));
    }

    #[test]
    fn rejects_nonpositive_or_invalid_process_ids() {
        for value in ["0", "-1", "all", "", "2147483648"] {
            assert!(parse_pid(value).is_err());
        }
    }

    #[test]
    fn detects_process_identity_changes() {
        assert!(verify_pair(&record(42, 1, 0), &record(42, 2, 0)).is_err());
        assert!(verify_pair(&record(42, 1, 0), &record(43, 1, 0)).is_err());
    }

    #[test]
    fn detects_decreasing_disk_counters() {
        assert!(verify_pair(&record(42, 1, 100), &record(42, 1, 99)).is_err());
    }

    #[test]
    fn accepts_monotonic_disk_counters() {
        assert!(verify_pair(&record(42, 1, 100), &record(42, 1, 100)).is_ok());
        assert!(verify_pair(&record(42, 1, 100), &record(42, 1, 101)).is_ok());
    }

    #[test]
    fn walks_descendants_without_duplicates_or_cycles() {
        let descendants = descendant_pids_with(1, |pid| {
            Ok(match pid {
                1 => vec![2, 3],
                2 => vec![3, 4],
                3 => vec![1],
                _ => Vec::new(),
            })
        })
        .unwrap();
        assert_eq!(descendants, [2, 3, 4]);
    }

    #[test]
    fn classifies_membership_churn_and_pid_reuse() {
        let before = [
            ProcessIdentity { pid: 10, start: 1 },
            ProcessIdentity { pid: 11, start: 2 },
            ProcessIdentity { pid: 12, start: 3 },
        ];
        let after = [
            ProcessIdentity { pid: 10, start: 1 },
            ProcessIdentity { pid: 11, start: 4 },
            ProcessIdentity { pid: 13, start: 5 },
        ];
        assert_eq!(
            compare_membership(&before, &after),
            MembershipDelta {
                added: vec![ProcessIdentity { pid: 13, start: 5 }],
                missing: vec![ProcessIdentity { pid: 12, start: 3 }],
                reused: vec![(
                    ProcessIdentity { pid: 11, start: 2 },
                    ProcessIdentity { pid: 11, start: 4 }
                )],
            }
        );
    }

    #[test]
    fn rejects_missing_child_ready_signal() {
        assert!(expect_line(&mut io::Cursor::new(Vec::<u8>::new()), "ready").is_err());
    }
}
