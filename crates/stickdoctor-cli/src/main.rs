use std::process::ExitCode;
use stickdoctor_core::{
    enumerate_disks, execute_repair, inspect_properties, new_logger, DiskCandidate, FsChoice,
    RepairOptions,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        None | Some("list") => cmd_list(),
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

fn print_help() {
    println!(
        "stickdoctor {}\n\n\
         Usage:\n  \
         stickdoctor-cli                    List physical disks\n  \
         stickdoctor-cli list               List physical disks\n  \
         stickdoctor-cli repair <N> [fs] [label] [options]\n\n\
         Arguments:\n  \
         <N>       Physical drive number (see `list`)\n  \
         [fs]      fat32 | exfat | ntfs   (default: fat32)\n  \
         [label]   Volume label, max 11 chars (default: STICK)\n\n\
         Options:\n  \
         --yes     Skip the confirmation prompt\n  \
         --zero    Zero the first/last 1 MiB before rebuilding (stubborn sticks)\n  \
         --dry-run Show what would happen, change nothing\n\n\
         Examples:\n  \
         stickdoctor-cli repair 3 fat32 MYSTICK\n  \
         stickdoctor-cli repair 3 exfat MYSTICK --yes\n",
        VERSION
    );
}

fn cmd_list() -> ExitCode {
    println!("StickDoctor {} - physical disks\n", VERSION);

    let devices = match enumerate_disks() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Cannot enumerate disks: {e}");
            return ExitCode::FAILURE;
        }
    };

    if devices.is_empty() {
        println!("No disks found. Run this program as Administrator.");
        return ExitCode::SUCCESS;
    }

    println!("IDX  BUS     SIZE        SAFETY      MODEL                       LAYOUT");
    println!("{}", "-".repeat(104));

    for dev in &devices {
        let c = DiskCandidate::probe(dev);
        let index = dev.drive_index.unwrap_or(0);
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
            "{:<4} {:<6} {:<11} {:<10} {:<26} {}",
            index,
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
    };

    let props = match inspect_properties(index) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Cannot inspect PhysicalDrive{}: {}", index, e);
            eprintln!("Run this program as Administrator.");
            return ExitCode::FAILURE;
        }
    };

    println!("Disk:   PhysicalDrive{}", index);
    println!(
        "Model:  {}",
        props.model.clone().unwrap_or_else(|| "Unknown".into())
    );
    println!("Bus:    {}", props.bus_type);
    println!("Size:   {:.2} GiB", props.size_gib());
    println!(
        "Writable: {}",
        if props.writable {
            "yes"
        } else {
            "NO (write-protected)"
        }
    );
    println!("Target: {}", fs);

    if let Some(warn) = fs.warn_for_size(props.size_gib()) {
        eprintln!("\nWARNING: {}", warn);
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
            println!("\nDone. The stick is formatted and should appear in Explorer.");
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
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
