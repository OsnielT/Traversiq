use serde_json::json;
use std::path::Path;
use std::process::ExitCode;
use traversiq::{api::trace_api, discovery::discover, load_workspace, source::trace};

const USAGE: &str = "Usage:\n  traversiq plugin configure <workspace.json> <local.json>\n  traversiq plugin mcp\n  traversiq plugin pre-edit\n  traversiq pre-edit <workspace.json> <local.json>\n  traversiq mcp <workspace.json> <local.json>\n  traversiq api-trace <workspace.json> <local.json> <provider-id> <schema-entry> <Type.field.path> [--all-warnings]\n  traversiq trace <workspace.json> <local.json> <provider-id> <source-entry> <symbol> [--all-warnings]\n  traversiq discover <projects-directory>\n  traversiq validate <workspace.json> <local.json>\n  traversiq impact <workspace.json> <local.json> <project-id>\n\nImpact reports declared project dependencies. Trace reports a partial JS/TS import graph. Pre-edit supports Codex apply_patch and Claude Edit/Write checkpoints with partial coverage.";

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("plugin") {
        match (args.get(1).map(String::as_str), args.len()) {
            (Some("configure"), 4) => println!(
                "{}",
                traversiq::plugin::configure(
                    &std::env::current_dir().map_err(|e| e.to_string())?,
                    Path::new(&args[2]),
                    Path::new(&args[3])
                )?
            ),
            (Some("pre-edit"), 2) => println!("{}", traversiq::plugin::pre_edit()),
            (Some("mcp"), 2) => {
                let settings = traversiq::plugin::settings(
                    &std::env::current_dir().map_err(|e| e.to_string())?,
                )?
                .ok_or("Traversiq is not configured here; run the Traversiq setup skill first")?;
                return serve(&settings.workspace, &settings.local);
            }
            _ => return Err(USAGE.into()),
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("pre-edit") && args.len() == 3 {
        println!(
            "{}",
            traversiq::checkpoint::run(Path::new(&args[1]), Path::new(&args[2]))
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("mcp") && args.len() == 3 {
        return serve(Path::new(&args[1]), Path::new(&args[2]));
    }
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("discover") && args.len() == 2 {
        let proposal = discover(Path::new(&args[1]))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&proposal).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("api-trace")
        && (args.len() == 6 || (args.len() == 7 && args[6] == "--all-warnings"))
    {
        let report = trace_api(
            Path::new(&args[1]),
            Path::new(&args[2]),
            &args[3],
            Path::new(&args[4]),
            &args[5],
            args.len() == 7,
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("trace")
        && (args.len() == 6 || (args.len() == 7 && args[6] == "--all-warnings"))
    {
        let report = trace(
            Path::new(&args[1]),
            Path::new(&args[2]),
            &args[3],
            Path::new(&args[4]),
            &args[5],
            args.len() == 7,
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let valid = matches!(args.first().map(String::as_str), Some("validate")) && args.len() == 3
        || matches!(args.first().map(String::as_str), Some("impact")) && args.len() == 4;
    if !valid {
        return Err(USAGE.into());
    }
    let workspace = load_workspace(Path::new(&args[1]), Path::new(&args[2]))?;
    let output = if args[0] == "impact" {
        let mut report =
            serde_json::to_value(workspace.impact(&args[3])?).map_err(|error| error.to_string())?;
        report["declaration_source"] = json!(args[1]);
        report
    } else {
        json!({"valid": true, "project_count": workspace.projects.len()})
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn serve(config: &Path, local: &Path) -> Result<(), String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(traversiq::mcp::serve(config, local))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("traversiq: {error}");
            ExitCode::FAILURE
        }
    }
}
