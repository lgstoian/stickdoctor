//! StickDoctor command line front end.
//!
//! @sd file module="crates/stickdoctor-cli/src/main.rs" role="CLI front end" entry="main" exit="cmd_repair"
//! @sd anchor name=cli.main text="CLI"
//! @sd ref name=core.lib

use std::process::ExitCode;
use stickdoctor_core::{
    enumerate_disks, execute_repair, inspect_properties, new_logger, CapacityTestMode,
    CapacityVerdict, DiskCandidate, FsChoice, RepairOptions,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        None | Some("list") => cmd_list(&args),
        Some("verify") => cmd_verify(&args[1..]),
        Some("scan") => cmd_scan(&args[1..]),
        Some("clear-readonly") => cmd_clear_ro(&args[1..]),
        Some("chkdsk") => cmd_chkdsk(&args[1..]),
        Some("capacity") => cmd_capacity(&args[1..]),
        Some("agent-map") => cmd_agent_map(&args[1..]),
        Some("repair") => cmd_repair(&args[1..]),
        Some("--version") | Some("-V") => {
            println!("stickdoctor {}", VERSION);
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") | Some("help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("Unknown command: {}", other);
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn cmd_verify(args: &[String]) -> ExitCode {
    let positionals: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let index: u32 = match positionals.first().and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => {
            eprintln!("Usage: stickdoctor-cli verify <N>");
            return ExitCode::FAILURE;
        }
    };

    println!("Verifying PhysicalDrive{}...", index);
    match stickdoctor_core::verify_disk(index) {
        Ok(Some(v)) => {
            let letter = v.drive_letter.as_deref().unwrap_or("?");
            let fs = v.filesystem.as_deref().unwrap_or("unknown");
            println!("  Volume: {}  filesystem: {}", letter, fs);
            println!("Result: OK - the stick is formatted and mounted.");
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("Result: NOT FORMATTED - no volume with a filesystem found.");
            println!("  Run: stickdoctor-cli repair {} fat32", index);
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("Cannot inspect disk {}: {}", index, e);
            ExitCode::FAILURE
        }
    }
}

fn cmd_scan(args: &[String]) -> ExitCode {
    let index = match positional_index(args) {
        Some(i) => i,
        None => {
            eprintln!("Usage: stickdoctor-cli scan <N>");
            return ExitCode::FAILURE;
        }
    };
    println!("Scanning PhysicalDrive{} for lost partitions...", index);
    let found = stickdoctor_core::scan_lost_partitions(index);
    if found.is_empty() {
        println!("No lost partitions found.");
    } else {
        for p in found {
            println!("  {} at LBA {} (signature {})", p.kind, p.lba, p.signature);
        }
    }
    ExitCode::SUCCESS
}

fn cmd_clear_ro(args: &[String]) -> ExitCode {
    let index = match positional_index(args) {
        Some(i) => i,
        None => {
            eprintln!("Usage: stickdoctor-cli clear-readonly <N>");
            return ExitCode::FAILURE;
        }
    };
    match stickdoctor_core::clear_read_only(index) {
        Ok(messages) => {
            for m in messages {
                println!("{}", m);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn cmd_chkdsk(args: &[String]) -> ExitCode {
    let positionals: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let letter = match positionals.first() {
        Some(l) => l.trim_end_matches(':').to_string(),
        None => {
            eprintln!("Usage: stickdoctor-cli chkdsk <DRIVE>");
            return ExitCode::FAILURE;
        }
    };
    // Never repair the filesystem that Windows is running from: `chkdsk /f` on
    // it requires a reboot and can disrupt the boot volume.
    if let Some(system) = stickdoctor_core::system_drive_letter() {
        if letter.eq_ignore_ascii_case(system.trim_end_matches(':')) {
            eprintln!("Refusing chkdsk on the Windows system drive ({}).", system);
            return ExitCode::FAILURE;
        }
    }
    match stickdoctor_core::run_chkdsk(&letter) {
        Ok(out) => {
            print!("{}", out);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("chkdsk failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn cmd_capacity(args: &[String]) -> ExitCode {
    let index = match positional_index(args) {
        Some(i) => i,
        None => {
            eprintln!("Usage: stickdoctor-cli capacity <N> [--quick|--full]");
            return ExitCode::FAILURE;
        }
    };
    let mode = if args.iter().any(|a| a == "--full") {
        CapacityTestMode::Full
    } else {
        CapacityTestMode::Quick
    };
    println!(
        "Testing capacity of PhysicalDrive{} ({} test; writes test data)...",
        index, mode
    );
    println!("WARNING: this destroys the volume contents. Back up first.");
    let log = stickdoctor_core::new_logger();
    match stickdoctor_core::verify_capacity_with_mode(index, mode, &log) {
        Ok(r) => {
            if let Ok(l) = log.lock() {
                let rendered = l.render();
                if !rendered.is_empty() {
                    println!("\n{}\n", rendered);
                }
            }
            match r.verdict {
                CapacityVerdict::Genuine => {
                    println!(
                        "Result: GENUINE - {} bytes verified at {} sample points.",
                        r.bytes_verified,
                        r.tested_offsets.len()
                    );
                    ExitCode::SUCCESS
                }
                CapacityVerdict::Counterfeit => {
                    println!(
                        "Result: COUNTERFEIT - {}",
                        r.error
                            .as_deref()
                            .unwrap_or("the advertised capacity is not real")
                    );
                    if let Some(w) = r.wrap_evidence {
                        println!(
                            "  Wrap evidence: the block at offset {} was written for offset {} (the real capacity divides {} bytes).",
                            w.read_offset, w.written_at, w.capacity_multiple
                        );
                    } else if let Some(off) = r.first_failure {
                        println!("  First failure at offset {}.", off);
                    }
                    ExitCode::FAILURE
                }
                CapacityVerdict::Corrupt => {
                    println!(
                        "Result: CORRUPT - {}",
                        r.error.as_deref().unwrap_or("the media changed the data")
                    );
                    ExitCode::FAILURE
                }
                CapacityVerdict::Inconclusive => {
                    println!(
                        "Result: INCONCLUSIVE - {}",
                        r.error.as_deref().unwrap_or("the test could not complete")
                    );
                    println!("  This is not proof of a fake card. Close programs using the drive and retry.");
                    ExitCode::FAILURE
                }
                CapacityVerdict::Cancelled => {
                    println!("Result: CANCELLED - the card may be partially overwritten.");
                    println!("  Run `repair` before using it.");
                    ExitCode::FAILURE
                }
            }
        }
        Err(e) => {
            eprintln!("Capacity test failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn cmd_agent_map(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let write = args.iter().any(|a| a == "--write");

    let map = match stickdoctor_agent::scan_workspace() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Cannot scan the workspace: {e}");
            return ExitCode::FAILURE;
        }
    };

    let errors = stickdoctor_agent::validate(&map);
    if !errors.is_empty() {
        eprintln!("Annotation errors ({}):", errors.len());
        for e in &errors {
            eprintln!("  {e}");
        }
        return ExitCode::FAILURE;
    }

    let output = if json {
        stickdoctor_agent::render_json(&map)
    } else {
        stickdoctor_agent::render_markdown(&map)
    };

    if write {
        let path = stickdoctor_agent::workspace_root()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("docs/AGENT_MAP.md");
        if let Err(e) = std::fs::write(&path, &output) {
            eprintln!("Cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("Wrote {}", path.display());
        return ExitCode::SUCCESS;
    }

    print!("{output}");
    ExitCode::SUCCESS
}

fn positional_index(args: &[String]) -> Option<u32> {
    args.iter()
        .find(|a| !a.starts_with("--"))
        .and_then(|s| s.parse().ok())
}

fn print_help() {
    println!(
        "stickdoctor {}\n\n\
         Usage:\n  \
         stickdoctor-cli [list] [--json]      List physical disks\n  \
         stickdoctor-cli agent-map [--json]   Agent breadcrumbs (machine-readable)\n  \
         stickdoctor-cli capacity <N> [options]  Counterfeit-capacity test (WRITES!)\n  \
         stickdoctor-cli repair <N> [fs] [label] [options]\n\n\
         Arguments:\n  \
         <N>       Physical drive number (see `list`)\n  \
         [fs]      fat32 | exfat | ntfs   (default: fat32)\n  \
         [label]   Volume label, max 11 chars (default: STICK)\n\n\
         Options:\n  \
         --json            Machine-readable output (list)\n  \
         --yes             Skip the confirmation prompt\n  \
         --zero            Zero the first/last 1 MiB before rebuilding\n  \
         --rebuild-only    Only rebuild the partition table, do not format\n  \
         --dry-run         Show what would happen, change nothing\n  \
         --full            Capacity test: write and verify the whole card (slow)\n  \
         --quick           Capacity test: sample 128 points (default, fast)\n\n\
         Examples:\n  \
         stickdoctor-cli list --json\n  \
         stickdoctor-cli capacity 3 --full\n  \
         stickdoctor-cli repair 3 fat32 MYSTICK\n  \
         stickdoctor-cli repair 3 exfat MYSTICK --yes\n",
        VERSION
    );
}

fn cmd_list(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let devices = match enumerate_disks() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Cannot enumerate disks: {e}");
            return ExitCode::FAILURE;
        }
    };

    let candidates: Vec<DiskCandidate> = devices.iter().map(DiskCandidate::probe).collect();

    if json {
        let disks: Vec<serde_json::Value> = candidates
            .iter()
            .map(|c| {
                serde_json::json!({
                    "index": c.device.drive_index,
                    "bus": c.properties.bus_type.to_string(),
                    "size_bytes": c.properties.size_bytes,
                    "size_gib": (c.properties.size_gib() * 10.0).round() / 10.0,
                    "removable": c.properties.removable,
                    "writable": c.properties.writable,
                    "model": c.properties.model,
                    "serial": c.properties.serial,
                    "partition_style": c.sector_zero.as_ref().map(|s| s.style.to_string()),
                    "safety": c.safety.label(),
                    "can_repair": c.can_repair(),
                    "volumes": c.volumes.iter().map(|v| serde_json::json!({
                        "letter": v.drive_letter,
                        "filesystem": v.filesystem,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let doc = serde_json::json!({ "version": VERSION, "disks": disks });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return ExitCode::SUCCESS;
    }

    println!("StickDoctor {} - physical disks\n", VERSION);
    if candidates.is_empty() {
        println!("No disks found. Run this program as Administrator.");
        return ExitCode::SUCCESS;
    }

    println!("IDX  MEDIA       BUS     SIZE        SAFETY      MODEL                       LAYOUT");
    println!("{}", "-".repeat(104));

    for c in &candidates {
        let index = c.device.drive_index.unwrap_or(0);
        let layout = c
            .sector_zero
            .as_ref()
            .map(|s| s.style.to_string())
            .unwrap_or_else(|| "(unreadable)".into());
        let model = c
            .properties
            .model
            .clone()
            .unwrap_or_else(|| "Unknown".into());
        println!(
            "{:<4} {:<12} {:<6} {:<11} {:<10} {:<26} {}",
            index,
            c.properties.media_type().to_string(),
            c.properties.bus_type.to_string(),
            format!("{:.1} GiB", c.properties.size_gib()),
            c.safety.label(),
            truncate(&model, 26),
            layout
        );
        if c.volumes.is_empty() {
            println!("     volumes: none visible to Windows");
        } else {
            for v in &c.volumes {
                println!(
                    "     volume: {}  fs={}",
                    v.drive_letter.as_deref().unwrap_or("(no letter)"),
                    v.filesystem.as_deref().unwrap_or("RAW")
                );
            }
        }
    }

    println!("\nRepair a stick:  stickdoctor-cli repair <IDX> [fat32|exfat|ntfs] [LABEL]");
    ExitCode::SUCCESS
}

fn cmd_repair(args: &[String]) -> ExitCode {
    let positionals: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let assume_yes = args.iter().any(|a| a == "--yes");
    let zero = args.iter().any(|a| a == "--zero");
    let rebuild_only = args.iter().any(|a| a == "--rebuild-only");
    let dry_run = args.iter().any(|a| a == "--dry-run");

    let index: u32 = match positionals.first().and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => {
            eprintln!("Usage: stickdoctor-cli repair <N> [fat32|exfat|ntfs] [LABEL]");
            return ExitCode::FAILURE;
        }
    };

    let fs = match positionals.get(1).map(|s| s.to_lowercase()).as_deref() {
        Some("exfat") => FsChoice::ExFat,
        Some("ntfs") => FsChoice::Ntfs,
        Some("fat32") | None => FsChoice::Fat32,
        Some(other) => {
            eprintln!("Unknown filesystem '{}'. Use fat32, exfat or ntfs.", other);
            return ExitCode::FAILURE;
        }
    };

    let label = positionals
        .get(2)
        .map(|s| s.to_string())
        .unwrap_or_default();
    let opts = RepairOptions {
        fs,
        label,
        quick: true,
        zero_sectors: zero,
        rebuild_only,
    };

    let props = match inspect_properties(index) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Cannot inspect PhysicalDrive{}: {}", index, e);
            eprintln!("Run this program as Administrator.");
            return ExitCode::FAILURE;
        }
    };

    // Refuse before the confirmation: the repair engine gates again, but the
    // user should not be asked to confirm an operation that cannot run.
    let safety = stickdoctor_core::safety_check(&props, index)
        .unwrap_or(stickdoctor_core::SafetyStatus::Unknown);
    if !matches!(safety, stickdoctor_core::SafetyStatus::Safe) {
        eprintln!("Refusing: {}", safety.long_label());
        return ExitCode::FAILURE;
    }

    println!("Disk:    PhysicalDrive{}", index);
    println!(
        "Model:   {}",
        props.model.clone().unwrap_or_else(|| "Unknown".into())
    );
    println!("Bus:     {}", props.bus_type);
    println!("Size:    {:.2} GiB", props.size_gib());
    println!(
        "Writable: {}",
        if props.writable {
            "yes"
        } else {
            "NO (write-protected)"
        }
    );
    println!(
        "Action:  {}",
        if rebuild_only {
            "rebuild partition table only".to_string()
        } else {
            format!("format as {}", fs)
        }
    );

    if !rebuild_only {
        if let Some(warn) = fs.warn_for_size(props.size_gib()) {
            if fs == FsChoice::Fat32 {
                eprintln!("\nWARNING: {}", warn);
            }
        }
    }

    if dry_run {
        println!("\n--dry-run: nothing was changed.");
        return ExitCode::SUCCESS;
    }

    if !assume_yes {
        println!("\nALL DATA on this disk will be destroyed. Type YES to continue:");
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        if input.trim() != "YES" {
            println!("Cancelled.");
            return ExitCode::SUCCESS;
        }
    }

    let log = new_logger();
    let result = execute_repair(index, &opts, &log);
    let rendered = log.lock().map(|l| l.render()).unwrap_or_default();
    println!("\n{}", rendered);

    match result {
        Ok(()) => {
            println!("\nDone.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("\nRepair failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // Char-based so a multi-byte model name cannot split a code point.
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("abc", 5), "abc");
    }

    #[test]
    fn truncate_shortens_long_strings() {
        assert_eq!(truncate("abcdefghij", 8), "abcde...");
    }

    #[test]
    fn truncate_handles_multibyte_model_names() {
        // A multi-byte name must not split a code point (the old code panicked).
        assert_eq!(truncate("µµµµµµµµµµµµ", 8), "µµµµµ...");
    }
}
