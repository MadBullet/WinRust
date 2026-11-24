//! `winrust-driver` - the kernel driver console.
//!
//! ```text
//! winrust-driver status            is the driver loaded, and what does it say?
//! winrust-driver install [image]   stage the .sys, register the service, start it
//! winrust-driver remove            stop, delete the service, delete the image
//! winrust-driver processes [n]     process table from *kernel* memory
//! winrust-driver modules [n]       loaded drivers with base addresses
//! winrust-driver perf              aggregate counters
//! winrust-driver blocked [name]    show or extend the process block list
//! winrust-driver json              everything above as one JSON document
//! ```
//!
//! Everything it prints comes from the driver; when the driver is not loaded the
//! tool says so plainly instead of falling back to user-mode data, so a caller
//! can tell the two apart.

mod client;
mod service;

use client::Driver;
use std::process::ExitCode;
use winrust_kernel_abi::{GuardAction, ModuleRecord, ProcessRecord};

fn usage() -> &'static str {
    "winrust-driver - WinRust kernel driver console

USAGE:
    winrust-driver <command>

COMMANDS:
    status              driver state, protocol version, CPU count
    install [path.sys]  stage and register the driver (default: ./WinRustKernel.sys)
    remove              stop and unregister the driver
    processes [limit]   process table straight from kernel memory
    modules [limit]     loaded kernel modules with base address and size
    perf                aggregate CPU / object counters
    block <name>        refuse future starts of this image (RAM only)
    allow <name>        stop refusing it
    unblock-all         clear the block list
    json                full snapshot as JSON (for scripting)

NOTES:
    install, remove and blocked need an elevated prompt.
    The driver is optional: without it every other WinRust component still works."
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        println!("{}", usage());
        return ExitCode::SUCCESS;
    };

    let result = match command {
        "status" => status(),
        "install" => install(args.get(1).map(String::as_str)),
        "remove" => remove(),
        "processes" => processes(parse_limit(&args)),
        "modules" => modules(parse_limit(&args)),
        "perf" => perf(),
        "blocked" => blocked(),
        "block" => guard(GuardAction::Block, args.get(1).map(String::as_str)),
        "allow" => guard(GuardAction::Allow, args.get(1).map(String::as_str)),
        "unblock-all" => guard(GuardAction::Clear, None),
        "json" => json(),
        "-h" | "--help" | "help" => {
            println!("{}", usage());
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}\n\n{}", usage());
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_limit(args: &[String]) -> usize {
    args.get(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(40)
}

fn status() -> Result<(), String> {
    let installed = service::is_installed();
    println!("service registered : {}", yes_no(installed));
    if service::reboot_pending() {
        println!("reboot pending     : yes (image swap deferred)");
    }
    match Driver::connect() {
        Ok(driver) => {
            let (version, processors, page_size) = driver.ping().map_err(|e| e.to_string())?;
            println!("driver reachable   : yes");
            println!("protocol version   : {version}");
            println!("processors         : {processors}");
            println!("page size          : {page_size} bytes");
            match driver.performance() {
                Ok(perf) => {
                    println!("processes          : {}", perf.process_count);
                    println!("threads            : {}", perf.thread_count);
                    println!("handles            : {}", perf.handle_count);
                }
                Err(e) => println!("counters           : unavailable ({e})"),
            }
            Ok(())
        }
        Err(e) => {
            println!("driver reachable   : no");
            println!();
            println!("{e}");
            Ok(())
        }
    }
}

fn install(path: Option<&str>) -> Result<(), String> {
    let source = path.unwrap_or("WinRustKernel.sys");
    let source = std::path::Path::new(source);
    if !source.is_file() {
        return Err(format!(
            "{} not found. Build it with `cargo make` in crates/winrust-kernel, or pass the path.",
            source.display()
        ));
    }
    let image = service::stage(source).map_err(|e| format!("staging failed: {e}"))?;
    println!("staged  : {}", image.display());

    if service::is_installed() {
        println!("service : already registered, restarting");
        let _ = service::stop();
    } else {
        service::install(&image).map_err(|e| format!("CreateService failed: {e}"))?;
        println!(
            "service : {SERVICE_NAME} registered",
            SERVICE_NAME = service::SERVICE_NAME
        );
    }
    service::start().map_err(|e| format!("start failed: {e}"))?;
    println!("started : driver is loading");

    match Driver::connect() {
        Ok(driver) => {
            let (version, processors, _) = driver.ping().map_err(|e| e.to_string())?;
            println!("verified: protocol {version}, {processors} processors");
            Ok(())
        }
        Err(e) => Err(format!(
            "the service started but the device is not reachable yet ({e}). \
             If this is the first install, reboot and try `winrust-driver status`."
        )),
    }
}

fn remove() -> Result<(), String> {
    if !service::is_installed() {
        println!("driver service is not registered");
        let _ = service::remove();
        return Ok(());
    }
    let _ = service::stop();
    service::remove().map_err(|e| format!("delete failed: {e}"))?;
    println!("driver stopped and unregistered");
    Ok(())
}

fn processes(limit: usize) -> Result<(), String> {
    let driver = Driver::connect().map_err(|e| e.to_string())?;
    let mut rows = driver.processes().map_err(|e| e.to_string())?;
    rows.sort_by_key(|row| std::cmp::Reverse(row.working_set));
    println!(
        "{:>6}  {:>6}  {:>4}  {:>5}  {:>10}  {:>8}  NAME",
        "PID", "PPID", "THR", "HND", "WORKING", "CPU"
    );
    for row in rows.iter().take(limit) {
        println!(
            "{:>6}  {:>6}  {:>4}  {:>5}  {:>10}  {:>8}  {}",
            row.pid,
            row.parent_pid,
            row.threads,
            row.handles,
            format_bytes(row.working_set),
            format_time(row.user_time + row.kernel_time),
            name_of(&row.name)
        );
    }
    println!("\n{} processes total (kernel view)", rows.len());
    Ok(())
}

fn modules(limit: usize) -> Result<(), String> {
    let driver = Driver::connect().map_err(|e| e.to_string())?;
    let mut rows = driver.modules().map_err(|e| e.to_string())?;
    rows.sort_by_key(|row| row.base_address);
    println!("{:>18}  {:>10}  IMAGE", "BASE", "SIZE");
    for row in rows.iter().take(limit) {
        println!(
            "{:>18}  {:>10}  {}",
            format!("{:#x}", row.base_address),
            format_bytes(u64::from(row.image_size)),
            name_of(&row.name)
        );
    }
    println!("\n{} modules loaded", rows.len());
    Ok(())
}

fn perf() -> Result<(), String> {
    let driver = Driver::connect().map_err(|e| e.to_string())?;
    let perf = driver.performance().map_err(|e| e.to_string())?;
    println!("processes        : {}", perf.process_count);
    println!("threads          : {}", perf.thread_count);
    println!("handles          : {}", perf.handle_count);
    println!(
        "CPU user/kernel  : {} / {}",
        format_time(perf.total_user_time),
        format_time(perf.total_kernel_time)
    );
    Ok(())
}

fn guard(action: GuardAction, name: Option<&str>) -> Result<(), String> {
    let name = name.unwrap_or("");
    if action != GuardAction::Clear && name.is_empty() {
        return Err("this command needs an image name, e.g. `notepad.exe`".into());
    }
    if action != GuardAction::Clear && (name.contains(['\\', '/']) || !name.is_ascii()) {
        return Err(format!(
            "{name:?} must be a bare ASCII image name, without a directory"
        ));
    }
    let driver = Driver::connect().map_err(|e| e.to_string())?;
    let status = driver.guard(action, name).map_err(|e| e.to_string())?;
    if status.active == 0 {
        println!("warning: the process guard is not registered in the driver");
    }
    match action {
        GuardAction::Block => println!("blocked {name:?}"),
        GuardAction::Allow => println!("allowed {name:?} again"),
        GuardAction::Clear => println!("block list cleared"),
    }
    println!("{} name(s) blocked", status.blocked_count);
    Ok(())
}

fn blocked() -> Result<(), String> {
    let driver = Driver::connect().map_err(|e| e.to_string())?;
    println!("the driver keeps the list in RAM; use `block <name>` to add entries");
    let perf = driver.performance().map_err(|e| e.to_string())?;
    println!(
        "{} processes currently visible from kernel mode",
        perf.process_count
    );
    Ok(())
}

fn json() -> Result<(), String> {
    let driver = Driver::connect().map_err(|e| e.to_string())?;
    let (version, processors, page_size) = driver.ping().map_err(|e| e.to_string())?;
    let processes = driver.processes().map_err(|e| e.to_string())?;
    let modules = driver.modules().map_err(|e| e.to_string())?;
    let perf = driver.performance().map_err(|e| e.to_string())?;

    let process_rows: Vec<serde_json::Value> = processes
        .iter()
        .map(|row: &ProcessRecord| {
            serde_json::json!({
                "pid": row.pid,
                "parent_pid": row.parent_pid,
                "threads": row.threads,
                "handles": row.handles,
                "session": row.session_id,
                "base_priority": row.base_priority,
                "user_time": row.user_time,
                "kernel_time": row.kernel_time,
                "working_set": row.working_set,
                "virtual_size": row.virtual_size,
                "name": name_of(&row.name),
            })
        })
        .collect();
    let module_rows: Vec<serde_json::Value> = modules
        .iter()
        .map(|row: &ModuleRecord| {
            serde_json::json!({
                "base": format!("{:#x}", row.base_address),
                "size": row.image_size,
                "name": name_of(&row.name),
            })
        })
        .collect();

    let doc = serde_json::json!({
        "protocol": version,
        "processors": processors,
        "page_size": page_size,
        "performance": {
            "processes": perf.process_count,
            "threads": perf.thread_count,
            "handles": perf.handle_count,
            "user_time": perf.total_user_time,
            "kernel_time": perf.total_kernel_time,
        },
        "processes": process_rows,
        "modules": module_rows,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn name_of(units: &[u16]) -> String {
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut index = 0;
    while value >= 1024.0 && index < UNITS.len() - 1 {
        value /= 1024.0;
        index += 1;
    }
    if index == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[index])
    }
}

/// CPU times arrive in 100 ns units, like `GetProcessTimes`.
fn format_time(units_100ns: i64) -> String {
    let total_seconds = (units_100ns.max(0) as f64) / 10_000_000.0;
    let hours = (total_seconds / 3600.0).floor();
    let minutes = ((total_seconds % 3600.0) / 60.0).floor();
    let seconds = total_seconds % 60.0;
    if hours > 0.0 {
        format!("{hours:.0}:{minutes:02.0}:{seconds:04.1}")
    } else {
        format!("{minutes:.0}:{seconds:04.1}")
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
