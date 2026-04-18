#![allow(dead_code)]

use tracing_subscriber::EnvFilter;

mod client;
mod command_log;
mod commands;
mod config;
mod error;
mod exit_codes;
mod models;
mod ocean;
mod output;
mod spectre;
#[cfg(test)]
mod tests;
mod transport;
mod tui;

use clap::{Parser, Subcommand, ValueEnum};
use output::{print_json, OutputFormat};

#[derive(Parser)]
#[command(
    name = "vcli",
    about = "Control Cadence Virtuoso from anywhere",
    long_about = "CLI tool for AI agents and humans to control Cadence Virtuoso, locally or remotely.\n\n\
        Examples:\n  \
        virtuoso tunnel start              # Start SSH tunnel\n  \
        virtuoso skill exec '1+1'          # Execute SKILL code\n  \
        virtuoso cell open --lib my --cell top\n  \
        virtuoso schema --all              # Show full command schema as JSON",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format: json or table (default: table in TTY, json in pipe)
    #[arg(long, global = true)]
    format: Option<FormatArg>,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    /// Suppress non-essential output
    #[arg(long, short, global = true)]
    quiet: bool,

    /// Enable debug logging
    #[arg(long, short, global = true)]
    verbose: bool,

    /// Connect to a specific Virtuoso session by ID (e.g. eda-meow-1).
    /// Use `virtuoso session list` to see available sessions.
    /// If omitted: auto-selects when only one session exists; errors if multiple.
    /// Also reads from VB_SESSION environment variable.
    #[arg(long, global = true)]
    session: Option<String>,

    /// Connection profile name (reads VB_REMOTE_HOST_<profile> etc.)
    #[arg(long, short, global = true)]
    profile: Option<String>,
}

#[derive(Clone, ValueEnum)]
enum FormatArg {
    Json,
    Table,
}

#[derive(Subcommand)]
enum Commands {
    /// Create .env template with default configuration
    #[command(
        long_about = "Create a .env configuration template in the current directory.\n\n\
            Examples:\n  \
            virtuoso init\n  \
            virtuoso init --if-not-exists"
    )]
    Init {
        /// Skip if .env already exists (exit 0 instead of error)
        #[arg(long)]
        if_not_exists: bool,
    },

    /// Manage SSH tunnel to remote Virtuoso host
    #[command(subcommand)]
    Tunnel(TunnelCmd),

    /// Execute SKILL code on connected Virtuoso instance
    #[command(subcommand)]
    Skill(SkillCmd),

    /// Manage cellviews in Virtuoso
    #[command(subcommand)]
    Cell(CellCmd),

    /// Circuit simulation automation via Ocean SKILL
    #[command(subcommand)]
    Sim(SimCmd),

    /// Process characterization (gm/Id lookup table generation)
    #[command(subcommand)]
    Process(ProcessCmd),

    /// Transistor sizing from gm/Id lookup tables
    #[command(subcommand)]
    Design(DesignCmd),

    /// Manage Maestro simulation sessions (ADE)
    #[command(subcommand)]
    Maestro(MaestroCmd),

    /// Create and edit schematics in Virtuoso
    #[command(subcommand)]
    Schematic(SchematicCmd),

    /// List and inspect active Virtuoso bridge sessions
    #[command(subcommand)]
    Session(SessionCmd),

    /// Show CLI command schema as JSON for agent introspection
    #[command(
        long_about = "Show the full command schema as JSON, useful for AI agent discovery.\n\n\
            Examples:\n  \
            virtuoso schema --all\n  \
            virtuoso schema tunnel start"
    )]
    Schema {
        /// Show full command tree
        #[arg(long)]
        all: bool,

        /// Command noun (e.g. tunnel)
        noun: Option<String>,

        /// Command verb (e.g. start)
        verb: Option<String>,
    },

    /// Manage Virtuoso windows and dialogs
    #[command(subcommand)]
    Window(WindowCmd),

    /// Interactive TUI dashboard
    Tui,
}

#[derive(Subcommand)]
enum TunnelCmd {
    /// Start SSH tunnel and deploy daemon
    #[command(
        long_about = "Establish SSH tunnel to remote host and deploy the bridge daemon.\n\n\
            Examples:\n  \
            virtuoso tunnel start\n  \
            virtuoso tunnel start --timeout 60\n  \
            virtuoso tunnel start --dry-run --format json"
    )]
    Start {
        /// Connection timeout in seconds
        #[arg(long, short, default_value = "30")]
        timeout: u64,

        /// Preview without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Stop SSH tunnel and clean up remote files
    #[command(
        long_about = "Stop the running SSH tunnel and optionally clean up remote files.\n\n\
            Examples:\n  \
            virtuoso tunnel stop\n  \
            virtuoso tunnel stop --force"
    )]
    Stop {
        /// Force kill even if PID verification fails
        #[arg(long)]
        force: bool,

        /// Preview without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Restart SSH tunnel (stop + start)
    Restart {
        /// Connection timeout in seconds
        #[arg(long, short, default_value = "30")]
        timeout: u64,
    },

    /// Show tunnel, daemon, and connection status
    #[command(
        long_about = "Check the status of tunnel, daemon, and Virtuoso connection.\n\n\
            Examples:\n  \
            virtuoso tunnel status\n  \
            virtuoso tunnel status --format json"
    )]
    Status,

    /// Run full connection diagnostics
    Diagnose,
}

#[derive(Subcommand)]
enum SkillCmd {
    /// Execute a SKILL expression and return result
    #[command(long_about = "Send a SKILL expression to Virtuoso for evaluation.\n\n\
            Examples:\n  \
            virtuoso skill exec '1+1'\n  \
            virtuoso skill exec 'geGetEditCellView()' --timeout 60")]
    Exec {
        /// SKILL expression to evaluate
        code: String,

        /// Execution timeout in seconds
        #[arg(long, short, default_value = "30")]
        timeout: u64,
    },

    /// Upload and load an IL script file into Virtuoso
    #[command(
        long_about = "Upload a SKILL/IL file to the remote host and load it.\n\n\
            Examples:\n  \
            virtuoso skill load my_script.il"
    )]
    Load {
        /// Path to .il file
        file: String,
    },
}

#[derive(Subcommand)]
enum CellCmd {
    /// Open a cellview for editing
    #[command(long_about = "Open a cellview in Virtuoso.\n\n\
            Examples:\n  \
            virtuoso cell open --lib myLib --cell myCell\n  \
            virtuoso cell open --lib myLib --cell myCell --view schematic --mode r")]
    Open {
        /// Library name
        #[arg(long)]
        lib: String,

        /// Cell name
        #[arg(long)]
        cell: String,

        /// View name
        #[arg(long, default_value = "layout")]
        view: String,

        /// Open mode: r(ead), o(verwrite), a(ppend)
        #[arg(long, default_value = "a")]
        mode: String,

        /// Preview without executing
        #[arg(long)]
        dry_run: bool,
    },

    /// Save the current cellview
    Save,

    /// Close the current cellview without saving
    Close,

    /// Get info about the currently open cellview
    Info,
}

#[derive(Subcommand)]
enum SimCmd {
    /// Set simulator and design target
    #[command(long_about = "Configure simulator and design for simulation.\n\n\
            Examples:\n  \
            virtuoso sim setup --lib FT0001A_SH --cell Bandgap_LDO\n  \
            virtuoso sim setup --lib myLib --cell myCell --simulator spectre")]
    Setup {
        /// Library name
        #[arg(long)]
        lib: String,

        /// Cell name
        #[arg(long)]
        cell: String,

        /// View name
        #[arg(long, default_value = "schematic")]
        view: String,

        /// Simulator engine
        #[arg(long, default_value = "spectre")]
        simulator: String,
    },

    /// Run a simulation analysis
    #[command(long_about = "Execute a simulation with specified analysis type.\n\n\
            Examples:\n  \
            virtuoso sim run --analysis tran --stop 10u\n  \
            virtuoso sim run --analysis dc --from 0 --to 1.2 --step 0.01\n  \
            virtuoso sim run --analysis ac --start 1 --stop 1e9 --dec 10")]
    Run {
        /// Analysis type: tran, dc, ac, stb
        #[arg(long)]
        analysis: String,

        /// Stop time (tran) or stop value (dc/ac)
        #[arg(long)]
        stop: Option<String>,

        /// Start value (dc/ac)
        #[arg(long)]
        start: Option<String>,

        /// From value (dc)
        #[arg(long)]
        from: Option<String>,

        /// To value (dc)
        #[arg(long)]
        to: Option<String>,

        /// Step value (dc)
        #[arg(long)]
        step: Option<String>,

        /// Points per decade (ac)
        #[arg(long)]
        dec: Option<String>,

        /// Error preset
        #[arg(long)]
        errpreset: Option<String>,

        /// Extra key=value params
        #[arg(long, value_parser = parse_key_val)]
        param: Vec<(String, String)>,

        /// Simulation timeout in seconds
        #[arg(long, short, default_value = "300")]
        timeout: u64,
    },

    /// Extract waveform measurements from last simulation
    #[command(long_about = "Extract metrics from simulation results.\n\n\
            Examples:\n  \
            virtuoso sim measure --expr 'ymax(VT(\"/OUT\"))'\n  \
            virtuoso sim measure --analysis tran --expr 'cross(VT(\"/OUT\") 0.6 1 \"rising\")'")]
    Measure {
        /// Measurement expression (can be repeated)
        #[arg(long, required = true)]
        expr: Vec<String>,

        /// Analysis type to select results from
        #[arg(long, default_value = "tran")]
        analysis: String,
    },

    /// Parameter sweep: vary a design variable and measure
    #[command(
        long_about = "Sweep a design variable across a range and collect measurements.\n\n\
            Examples:\n  \
            virtuoso sim sweep --var W --from 1e-6 --to 5e-6 --step 1e-6 \\\n    \
              --measure 'ymax(VT(\"/OUT\"))'"
    )]
    Sweep {
        /// Design variable to sweep
        #[arg(long)]
        var: String,

        /// Start value
        #[arg(long)]
        from: f64,

        /// End value
        #[arg(long)]
        to: f64,

        /// Step size
        #[arg(long)]
        step: f64,

        /// Measurement expression (can be repeated)
        #[arg(long, required = true)]
        measure: Vec<String>,

        /// Analysis type
        #[arg(long, default_value = "tran")]
        analysis: String,

        /// Simulation timeout in seconds
        #[arg(long, short, default_value = "600")]
        timeout: u64,
    },

    /// PVT corner analysis from JSON config
    #[command(
        long_about = "Run simulations across PVT corners defined in a JSON file.\n\n\
            Examples:\n  \
            virtuoso sim corner --file corners.json\n  \
            virtuoso sim corner --file corners.json --format table"
    )]
    Corner {
        /// Path to corner configuration JSON file
        #[arg(long)]
        file: String,

        /// Simulation timeout in seconds (per corner)
        #[arg(long, short, default_value = "600")]
        timeout: u64,
    },

    /// Show simulation results directory and contents
    Results,

    /// Regenerate simulation netlist (self-contained: sets up session then exports)
    #[command(
        long_about = "Set up the Ocean session and regenerate the Spectre netlist.\n\
            Does not require a prior `sim setup` or open ADE window.\n\n\
            Examples:\n  \
            virtuoso sim netlist --lib FT0001A_SH --cell ota5t\n  \
            virtuoso sim netlist --lib FT0001A_SH --cell ota5t --recreate"
    )]
    Netlist {
        /// Library name
        #[arg(long)]
        lib: String,

        /// Cell name
        #[arg(long)]
        cell: String,

        /// View name
        #[arg(long, default_value = "schematic")]
        view: String,

        /// Force full netlist recreation (clears stale cache)
        #[arg(long)]
        recreate: bool,

        /// Append analysis block(s) for standalone Spectre (dc, ac, tran).
        /// Also auto-fixes ADE OA-relative model paths (SFE-868).
        /// Can be specified multiple times: --with-analysis dc --with-analysis ac
        #[arg(long = "with-analysis", value_name = "TYPE")]
        with_analysis: Vec<String>,
    },

    /// Launch simulation asynchronously (returns job ID)
    RunAsync {
        /// Path to netlist file (.scs)
        #[arg(long)]
        netlist: String,
    },

    /// Check status of an async simulation job
    JobStatus {
        /// Job ID
        id: String,
    },

    /// List all simulation jobs
    JobList,

    /// Cancel a running simulation job
    JobCancel {
        /// Job ID
        id: String,
    },
}

#[derive(Subcommand)]
enum ProcessCmd {
    /// Characterize a process node (generate gm/Id lookup tables)
    #[command(
        long_about = "Sweep VGS × L on a single-transistor testbench to generate gm/Id lookup tables.\n\n\
            Examples:\n  \
            virtuoso process char --lib FT0001A_SH --cell gmid --inst /NM0 --type nmos\n  \
            virtuoso process char --lib myLib --cell gmid_p --inst /PM0 --type pmos --output process_data/myPDK"
    )]
    Char {
        /// Library name (unused in --netlist mode)
        #[arg(long, default_value = "")]
        lib: String,
        /// Cell name (unused in --netlist mode)
        #[arg(long, default_value = "")]
        cell: String,
        /// View name
        #[arg(long, default_value = "schematic")]
        view: String,
        /// Instance path (e.g. /NM0 or /PM0)
        #[arg(long, default_value = "/NM0")]
        inst: String,
        /// Device type: nmos or pmos
        #[arg(long, default_value = "nmos")]
        r#type: String,
        /// L values to sweep (comma-separated, in meters)
        #[arg(long, default_value = "200e-9,500e-9,1e-6")]
        l_values: String,
        /// VGS start voltage (VSG for pmos in --netlist mode)
        #[arg(long, default_value = "0.3")]
        vgs_start: f64,
        /// VGS stop voltage
        #[arg(long, default_value = "1.1")]
        vgs_stop: f64,
        /// VGS step voltage
        #[arg(long, default_value = "0.05")]
        vgs_step: f64,
        /// Output directory for lookup JSON
        #[arg(long, default_value = "process_data/default")]
        output: String,
        /// Timeout per simulation point
        #[arg(long, short, default_value = "60")]
        timeout: u64,
        /// Use direct Spectre netlist (no Virtuoso session required)
        #[arg(long)]
        netlist: bool,
        /// Model file path (required for --netlist mode)
        #[arg(long, default_value = "")]
        model_file: String,
        /// Model section (e.g. tt, ff, ss)
        #[arg(long, default_value = "tt")]
        model_section: String,
        /// Supply voltage (VDD) for netlist mode
        #[arg(long, default_value = "1.2")]
        vdd: f64,
        /// Spectre model name for NMOS device (PDK-specific, e.g. n12, nfet_01v8, nch)
        #[arg(long, default_value = "n12")]
        nmos_model: String,
        /// Spectre model name for PMOS device (PDK-specific, e.g. p12, pfet_01v8, pch)
        #[arg(long, default_value = "p12")]
        pmos_model: String,
        /// Instance name in netlist (default: NM0 for nmos, PM0 for pmos)
        #[arg(long)]
        inst_name: Option<String>,
        /// Saturation bias VDS/VSD (default: 0.6V)
        #[arg(long, default_value = "0.6")]
        vds: f64,
    },
}

#[derive(Subcommand)]
enum DesignCmd {
    /// Size a transistor from gm/Id lookup table
    #[command(
        long_about = "Calculate W/L from gm or Id requirement using process lookup table.\n\n\
            Examples:\n  \
            virtuoso design size --gmid 14 --l 500e-9 --gm 188e-6 --pdk smic13mmrf\n  \
            virtuoso design size --gmid 10 --l 1e-6 --id 50e-6 --pdk smic13mmrf --type pmos"
    )]
    Size {
        /// Target gm/Id value
        #[arg(long)]
        gmid: f64,
        /// Channel length (meters)
        #[arg(long)]
        l: f64,
        /// Required gm (S) — calculates W from this
        #[arg(long)]
        gm: Option<f64>,
        /// Required Id (A) — alternative to gm
        #[arg(long)]
        id: Option<f64>,
        /// PDK name (must have lookup in process_data/)
        #[arg(long, default_value = "smic13mmrf")]
        pdk: String,
        /// Device type: nmos or pmos
        #[arg(long, default_value = "nmos")]
        r#type: String,
    },

    /// Explore gm/Id design space for a process
    #[command(
        long_about = "Display full gm/Id lookup table for a process/device.\n\n\
            Examples:\n  \
            virtuoso design explore --pdk smic13mmrf\n  \
            virtuoso design explore --pdk smic13mmrf --type pmos"
    )]
    Explore {
        /// PDK name
        #[arg(long, default_value = "smic13mmrf")]
        pdk: String,
        /// Device type
        #[arg(long, default_value = "nmos")]
        r#type: String,
    },
}

#[derive(Subcommand)]
enum MaestroCmd {
    /// Open a Maestro session (background mode)
    Open {
        #[arg(long)]
        lib: String,
        #[arg(long)]
        cell: String,
        #[arg(long, default_value = "maestro")]
        view: String,
    },

    /// Close a Maestro session
    Close {
        /// Session ID (e.g. fnxSession4)
        #[arg(long)]
        session: String,
    },

    /// List all active Maestro sessions
    ListSessions,

    /// Set a design variable
    SetVar {
        #[arg(long)]
        session: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        value: String,
    },

    /// Get enabled analyses
    GetAnalyses {
        #[arg(long)]
        session: String,
    },

    /// Enable an analysis type on a setup (e.g. ac, dc, tran, noise)
    SetAnalysis {
        #[arg(long)]
        session: String,
        /// Analysis type: ac | dc | tran | noise | ...
        #[arg(long)]
        analysis: String,
    },

    /// Add an output expression
    AddOutput {
        #[arg(long)]
        session: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        expr: String,
    },

    /// Run simulation (async, returns immediately)
    Run {
        #[arg(long)]
        session: String,
    },

    /// Save Maestro setup to disk
    Save {
        #[arg(long)]
        session: String,
    },

    /// Export results to CSV
    Export {
        #[arg(long)]
        session: String,
        /// Output CSV file path
        #[arg(long)]
        path: String,
    },
}

#[derive(Subcommand)]
enum SchematicCmd {
    /// Open or create a schematic cellview for editing
    Open {
        #[arg(long)]
        lib: String,
        #[arg(long)]
        cell: String,
        #[arg(long, default_value = "schematic")]
        view: String,
    },

    /// Place an instance in the schematic
    Place {
        /// Master cell in lib/cell format (e.g. smic13mmrf/p12)
        #[arg(long)]
        master: String,
        /// Instance name
        #[arg(long)]
        name: String,
        /// X coordinate
        #[arg(long, default_value = "0")]
        x: i64,
        /// Y coordinate
        #[arg(long, default_value = "0")]
        y: i64,
        /// Orientation
        #[arg(long, value_enum, default_value_t = commands::schematic::Orient::R0)]
        orient: commands::schematic::Orient,
        /// Instance parameters as key=value pairs (e.g. w=14u,l=1u)
        #[arg(long)]
        params: Option<String>,
    },

    /// Create a wire between coordinates
    Wire {
        #[arg(long)]
        net: String,
        /// Points as x1,y1 x2,y2 ...
        #[arg(required = true)]
        points: Vec<String>,
    },

    /// Connect two instance terminals with a named net
    Conn {
        #[arg(long)]
        net: String,
        /// Source terminal (inst:term)
        #[arg(long)]
        from: String,
        /// Destination terminal (inst:term)
        #[arg(long)]
        to: String,
    },

    /// Add a net label
    Label {
        #[arg(long)]
        net: String,
        #[arg(long, default_value = "0")]
        x: i64,
        #[arg(long, default_value = "0")]
        y: i64,
    },

    /// Add a pin
    Pin {
        #[arg(long)]
        net: String,
        /// Pin direction: input, output, inputOutput
        #[arg(long)]
        dir: String,
        #[arg(long, default_value = "0")]
        x: i64,
        #[arg(long, default_value = "0")]
        y: i64,
    },

    /// Run schematic check (schCheck)
    Check,

    /// Save current schematic
    Save,

    /// Build schematic from JSON spec file
    Build {
        /// Path to JSON spec file
        #[arg(long)]
        spec: String,
    },

    /// List all instances in the open cellview
    ListInstances,

    /// List all nets in the open cellview
    ListNets,

    /// List all pins (terminals) in the open cellview
    ListPins,

    /// Get parameters of a specific instance
    GetParams {
        /// Instance name (e.g. M1)
        #[arg(long)]
        inst: String,
    },
}

#[derive(Subcommand)]
enum SessionCmd {
    /// List all active Virtuoso bridge sessions
    #[command(long_about = "Show all registered Virtuoso sessions.\n\n\
            Each Virtuoso instance running RBStart() registers a session file.\n\
            Use the session ID with --session to connect to a specific instance.\n\n\
            Examples:\n  \
            virtuoso session list\n  \
            virtuoso session list --format json")]
    List,

    /// Show details for a specific session
    Show {
        /// Session ID (e.g. eda-meow-1)
        id: String,
    },
}

#[derive(Subcommand)]
enum WindowCmd {
    /// List all open Virtuoso windows with their names and derived mode
    List,

    /// Dismiss the currently active blocking dialog
    DismissDialog {
        /// Action to take: cancel (default) or ok
        #[arg(long, default_value = "cancel")]
        action: String,
        /// Report dialog name without clicking
        #[arg(long)]
        dry_run: bool,
    },

    /// Capture a screenshot of the current Virtuoso window (IC23.1+)
    Screenshot {
        /// Output file path (PNG)
        #[arg(long)]
        path: String,
        /// Match window by name pattern (regex); uses current window if omitted
        #[arg(long)]
        window: Option<String>,
    },
}

fn parse_key_val(s: &str) -> std::result::Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no '=' in '{s}'"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

// ── Per-group dispatch helpers ───────────────────────────────────────

fn dispatch_tunnel(cmd: TunnelCmd, format: OutputFormat) -> error::Result<serde_json::Value> {
    match cmd {
        TunnelCmd::Start { timeout, dry_run } => commands::tunnel::start(Some(timeout), dry_run),
        TunnelCmd::Stop { force, dry_run } => commands::tunnel::stop(force, dry_run),
        TunnelCmd::Restart { timeout } => commands::tunnel::restart(Some(timeout)),
        TunnelCmd::Status => commands::tunnel::status(format),
        TunnelCmd::Diagnose => commands::tunnel::diagnose(),
    }
}

fn dispatch_skill(cmd: SkillCmd) -> error::Result<serde_json::Value> {
    match cmd {
        SkillCmd::Exec { code, timeout } => commands::skill::exec(&code, timeout),
        SkillCmd::Load { file } => commands::skill::load(&file),
    }
}

fn dispatch_cell(cmd: CellCmd) -> error::Result<serde_json::Value> {
    match cmd {
        CellCmd::Open {
            lib,
            cell,
            view,
            mode,
            dry_run,
        } => commands::cell::open(&lib, &cell, &view, &mode, dry_run),
        CellCmd::Save => commands::cell::save(),
        CellCmd::Close => commands::cell::close(),
        CellCmd::Info => commands::cell::info(),
    }
}

fn dispatch_sim(cmd: SimCmd) -> error::Result<serde_json::Value> {
    match cmd {
        SimCmd::Setup {
            lib,
            cell,
            view,
            simulator,
        } => commands::sim::setup(&lib, &cell, &view, &simulator),
        SimCmd::Run {
            analysis,
            stop,
            start,
            from,
            to,
            step,
            dec,
            errpreset,
            param,
            timeout,
        } => {
            let mut params: std::collections::HashMap<String, String> = param.into_iter().collect();
            if let Some(v) = stop {
                params.insert("stop".into(), v);
            }
            if let Some(v) = start {
                params.insert("start".into(), v);
            }
            if let Some(v) = from {
                params.insert("from".into(), v);
            }
            if let Some(v) = to {
                params.insert("to".into(), v);
            }
            if let Some(v) = step {
                params.insert("step".into(), v);
            }
            if let Some(v) = dec {
                params.insert("dec".into(), v);
            }
            if let Some(v) = errpreset {
                params.insert("errpreset".into(), v);
            }
            commands::sim::run(&analysis, &params, timeout)
        }
        SimCmd::Measure { expr, analysis } => commands::sim::measure(&analysis, &expr),
        SimCmd::Sweep {
            var,
            from,
            to,
            step,
            measure,
            analysis,
            timeout,
        } => commands::sim::sweep(&var, from, to, step, &analysis, &measure, timeout),
        SimCmd::Corner { file, timeout } => commands::sim::corner(&file, timeout),
        SimCmd::Results => commands::sim::results(),
        SimCmd::Netlist {
            lib,
            cell,
            view,
            recreate,
            with_analysis,
        } => commands::sim::netlist(&lib, &cell, &view, recreate, &with_analysis),
        SimCmd::RunAsync { netlist } => commands::sim::run_async(&netlist),
        SimCmd::JobStatus { id } => commands::sim::job_status(&id),
        SimCmd::JobList => commands::sim::job_list(),
        SimCmd::JobCancel { id } => commands::sim::job_cancel(&id),
    }
}

fn dispatch_process(cmd: ProcessCmd) -> error::Result<serde_json::Value> {
    match cmd {
        ProcessCmd::Char {
            lib,
            cell,
            view,
            inst,
            r#type,
            l_values,
            vgs_start,
            vgs_stop,
            vgs_step,
            output,
            timeout,
            netlist,
            model_file,
            model_section,
            vdd,
            nmos_model,
            pmos_model,
            inst_name,
            vds,
        } => {
            let l_vals: Vec<f64> = l_values
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if netlist {
                let device_model = if r#type == "pmos" {
                    &pmos_model
                } else {
                    &nmos_model
                };
                let resolved_inst = inst_name.unwrap_or_else(|| {
                    if r#type == "pmos" {
                        "PM0".into()
                    } else {
                        "NM0".into()
                    }
                });
                commands::process::char_netlist(
                    &r#type,
                    &l_vals,
                    vgs_start,
                    vgs_stop,
                    vgs_step,
                    &output,
                    &model_file,
                    &model_section,
                    vdd,
                    device_model,
                    &resolved_inst,
                    vds,
                )
            } else {
                commands::process::char(
                    &lib, &cell, &view, &inst, &r#type, &l_vals, vgs_start, vgs_stop, vgs_step,
                    &output, timeout,
                )
            }
        }
    }
}

fn dispatch_design(cmd: DesignCmd, format: OutputFormat) -> error::Result<serde_json::Value> {
    match cmd {
        DesignCmd::Size {
            gmid,
            l,
            gm,
            id,
            pdk,
            r#type,
        } => commands::design::size(gmid, l, gm, id, &pdk, &r#type, format),
        DesignCmd::Explore { pdk, r#type } => commands::design::explore(&pdk, &r#type, format),
    }
}

fn dispatch_maestro(cmd: MaestroCmd) -> error::Result<serde_json::Value> {
    match cmd {
        MaestroCmd::Open { lib, cell, view } => commands::maestro::open(&lib, &cell, &view),
        MaestroCmd::Close { session } => commands::maestro::close(&session),
        MaestroCmd::ListSessions => commands::maestro::list_sessions(),
        MaestroCmd::SetVar {
            session,
            name,
            value,
        } => commands::maestro::set_var(&session, &name, &value),
        MaestroCmd::GetAnalyses { session } => commands::maestro::get_analyses(&session),
        MaestroCmd::SetAnalysis { session, analysis } => {
            commands::maestro::set_analysis(&session, &analysis)
        }
        MaestroCmd::AddOutput {
            session,
            name,
            expr,
        } => commands::maestro::add_output(&session, &name, &expr),
        MaestroCmd::Run { session } => commands::maestro::run(&session),
        MaestroCmd::Save { session } => commands::maestro::save(&session),
        MaestroCmd::Export { session, path } => commands::maestro::export(&session, &path),
    }
}

fn dispatch_schematic(cmd: SchematicCmd) -> error::Result<serde_json::Value> {
    match cmd {
        SchematicCmd::Open { lib, cell, view } => commands::schematic::open(&lib, &cell, &view),
        SchematicCmd::Place {
            master,
            name,
            x,
            y,
            orient,
            params,
        } => {
            let param_pairs: Vec<(String, String)> = params
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .filter_map(|s| {
                    let (k, v) = s.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect();
            commands::schematic::place(&master, &name, x, y, orient, &param_pairs)
        }
        SchematicCmd::Wire { net, points } => commands::schematic::wire_from_strings(&net, &points),
        SchematicCmd::Conn { net, from, to } => commands::schematic::conn(&net, &from, &to),
        SchematicCmd::Label { net, x, y } => commands::schematic::label(&net, x, y),
        SchematicCmd::Pin { net, dir, x, y } => commands::schematic::pin(&net, &dir, x, y),
        SchematicCmd::Check => commands::schematic::check(),
        SchematicCmd::Save => commands::schematic::save(),
        SchematicCmd::Build { spec } => commands::schematic::build(&spec),
        SchematicCmd::ListInstances => commands::schematic::list_instances(),
        SchematicCmd::ListNets => commands::schematic::list_nets(),
        SchematicCmd::ListPins => commands::schematic::list_pins(),
        SchematicCmd::GetParams { inst } => commands::schematic::get_params(&inst),
    }
}

fn dispatch_window(cmd: WindowCmd) -> error::Result<serde_json::Value> {
    match cmd {
        WindowCmd::List => commands::window::list(),
        WindowCmd::DismissDialog { action, dry_run } => {
            commands::window::dismiss_dialog(&action, dry_run)
        }
        WindowCmd::Screenshot { path, window } => {
            commands::window::screenshot(&path, window.as_deref())
        }
    }
}

fn main() {
    let cli = Cli::parse();

    // Propagate profile to config layer via env var
    if let Some(ref profile) = cli.profile {
        std::env::set_var("VB_PROFILE", profile);
    }

    let log_level = if cli.verbose {
        "debug"
    } else if cli.quiet {
        "error"
    } else {
        "info"
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level)),
        )
        .with_target(false)
        .init();

    let format = match &cli.format {
        Some(FormatArg::Json) => OutputFormat::Json,
        Some(FormatArg::Table) => OutputFormat::Table,
        None => OutputFormat::resolve(None),
    };

    // Propagate --session so VirtuosoClient::from_env() picks it up.
    //
    // Because --session is a clap global arg, it can be captured even when the user
    // passes it as a subcommand-local arg (e.g. `vcli maestro get-analyses --session fnxSession3`).
    // To avoid clobbering a bridge session already set via VB_SESSION, we only write
    // to VB_SESSION when VB_SESSION is not already present in the environment.
    // Users who want to select the bridge session explicitly should pass `--session`
    // BEFORE the subcommand name, or set VB_SESSION in the environment.
    let session_from_env = std::env::var("VB_SESSION").ok();
    if session_from_env.is_none() {
        if let Some(ref s) = cli.session {
            std::env::set_var("VB_SESSION", s);
        }
    }

    let is_status_cmd = matches!(&cli.command, Commands::Tunnel(TunnelCmd::Status));

    // Early-exit commands that manage their own output
    match &cli.command {
        Commands::Schema { all, noun, verb } => {
            let schema = if *all || noun.is_none() {
                commands::schema::show(None, None)
            } else {
                commands::schema::show(noun.as_deref(), verb.as_deref())
            };
            print_json(&schema);
            return;
        }
        Commands::Tui => {
            if let Err(e) = tui::run_tui() {
                eprintln!("TUI error: {e}");
                std::process::exit(1);
            }
            return;
        }
        _ => {}
    }

    let result = match cli.command {
        Commands::Init { if_not_exists } => commands::init::run(if_not_exists),
        Commands::Tunnel(cmd) => dispatch_tunnel(cmd, format),
        Commands::Skill(cmd) => dispatch_skill(cmd),
        Commands::Cell(cmd) => dispatch_cell(cmd),
        Commands::Sim(cmd) => dispatch_sim(cmd),
        Commands::Process(cmd) => dispatch_process(cmd),
        Commands::Design(cmd) => dispatch_design(cmd, format),
        Commands::Maestro(cmd) => dispatch_maestro(cmd),
        Commands::Schematic(cmd) => dispatch_schematic(cmd),
        Commands::Session(cmd) => match cmd {
            SessionCmd::List => commands::session::list(format),
            SessionCmd::Show { id } => commands::session::show(&id, format),
        },
        Commands::Window(cmd) => dispatch_window(cmd),
        // Already handled above; unreachable but required for exhaustive match
        Commands::Schema { .. } | Commands::Tui => unreachable!(),
    };

    match result {
        Ok(value) => {
            let exit_code = if value
                .get("dry_run")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                exit_codes::DRY_RUN_OK
            } else {
                exit_codes::SUCCESS
            };

            match format {
                OutputFormat::Json => print_json(&value),
                OutputFormat::Table => {
                    if !is_status_cmd {
                        output::print_value(&value, format);
                    }
                }
            }

            std::process::exit(exit_code);
        }
        Err(e) => {
            let cli_error = e.to_cli_error();
            cli_error.print(format);
            std::process::exit(e.exit_code());
        }
    }
}
