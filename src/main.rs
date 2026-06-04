use tracing_subscriber::EnvFilter;

mod async_runtime;
mod auth;
mod capability;
mod client;
mod command_log;
mod commands;
mod config;
mod error;
mod exit_codes;
mod history;
mod mcp;
mod models;
mod ocean;
mod output;
mod plugins;
mod rpc;
mod skill_finder;
mod spectre;
mod streaming;
#[cfg(test)]
mod tests;
mod transaction;
mod transport;
mod tui;
mod version;

pub use auth::Auth;
pub use rpc::schema::standard_schema;
pub use transaction::{SchematicDiff, SchematicSnapshot, TransactionManager};

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

    /// Transaction management — begin/commit/rollback/diff snapshots of schematic changes
    #[command(subcommand)]
    Tx(TxCmd),

    /// Typed RPC — call methods by name with JSON params (AI agent interface)
    #[command(subcommand)]
    Rpc(RpcCmd),

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

    /// Start stdio-based MCP server for AI agent integration
    #[command(subcommand)]
    Mcp(McpCmd),

    /// Show or edit connection profile bindings
    #[command(subcommand)]
    Profile(ProfileCmd),
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// Show the resolved profile and its source
    #[command(
        long_about = "Print the connection profile that Config::from_env() would resolve, \
            plus the source layer it came from.\n\n\
            Resolution order:\n  \
            1. VB_PROFILE env var\n  \
            2. $VIRTUAL_ENV/.vcli-profile (Python venv binding)\n  \
            3. ~/.vcli/.env VB_PROFILE=... (user-level default)\n\n\
            Examples:\n  \
            virtuoso profile show\n  \
            virtuoso profile show --format json"
    )]
    Show,

    /// Bind a profile name to a scope (write the binding file)
    #[command(
        long_about = "Bind a profile name to a scope so subsequent vcli invocations resolve \
            that profile automatically. One of --venv, --user, or --local is required.\n\n\
            Scopes:\n  \
            --venv : write $VIRTUAL_ENV/.vcli-profile  (project Python venv)\n  \
            --user : write ~/.vcli/.env VB_PROFILE=...  (user-level default)\n  \
            --local: write ./.vcli-profile  (current working dir)\n\n\
            Examples:\n  \
            virtuoso profile bind t28_digital --venv\n  \
            virtuoso profile bind analog_default --user"
    )]
    Bind {
        /// Profile name to bind (e.g. "t28_digital", "analog_default")
        name: String,

        /// Bind to $VIRTUAL_ENV/.vcli-profile
        #[arg(long, conflicts_with_all = &["user", "local"])]
        venv: bool,

        /// Bind to ~/.vcli/.env VB_PROFILE=
        #[arg(long, conflicts_with_all = &["venv", "local"])]
        user: bool,

        /// Bind to ./.vcli-profile (current working dir)
        #[arg(long, conflicts_with_all = &["venv", "user"])]
        local: bool,
    },

    /// Clear a profile binding (remove the file or relevant line)
    #[command(
        long_about = "Clear a profile binding. One of --venv, --user, or --local is required.\n\n\
            Examples:\n  \
            virtuoso profile clear --venv\n  \
            virtuoso profile clear --user"
    )]
    Clear {
        /// Clear $VIRTUAL_ENV/.vcli-profile
        #[arg(long, conflicts_with_all = &["user", "local"])]
        venv: bool,

        /// Clear ~/.vcli/.env VB_PROFILE= line
        #[arg(long, conflicts_with_all = &["venv", "local"])]
        user: bool,

        /// Clear ./.vcli-profile
        #[arg(long, conflicts_with_all = &["venv", "user"])]
        local: bool,
    },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Run the MCP server (stdio mode)
    Serve,
}

impl McpCmd {
    fn dispatch(&self) -> error::Result<serde_json::Value> {
        // Initialize auth before serving (reads VCLI_API_KEY from env)
        Auth::init();
        match self {
            McpCmd::Serve => crate::mcp::server::run().map(|_| serde_json::json!({})),
        }
    }
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

        /// Sandbox/readonly mode — blocks system/sh/evalstring and other dangerous patterns
        #[arg(long)]
        readonly: bool,
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

    /// Execute a SKILL expression across all live sessions concurrently
    #[command(
        long_about = "Run a SKILL expression on every live local session in parallel.\n\n\
            Each session gets its own connection; results are collected into a JSON array.\
            \n\
            Exit code is non-zero only when every session fails.\n\n\
            Examples:\n  \
            virtuoso skill broadcast 'getVersion(t)'\n  \
            virtuoso skill broadcast 'geGetEditCellView()~>cellName'"
    )]
    Broadcast {
        /// SKILL expression to evaluate on all sessions
        code: String,

        /// Execution timeout in seconds (per session)
        #[arg(long, short, default_value = "30")]
        timeout: u64,
    },

    /// Execute inline SKILL one-liners (companion to `load` for round-trip checks)
    #[command(
        long_about = "Execute inline SKILL expressions — companion to `load` for one-liners.\n\n\
            Two input modes:\n\
            \n\
            virtuoso skill eval 'getCurrentTime()'\n\
            echo 'expr' | virtuoso skill eval --stdin\n\n\
            --stdin sidesteps shell quoting for snippets with embedded quotes, parens, or\n\
            quoted symbols, and is the natural way to feed multi-line SKILL via heredoc.\n\n\
            Multi-statement input is supported transparently — the expression is wrapped in\n\
            `progn(...)` before sending, and the value of the last form is returned.\n\n\
            Output: full VirtuosoResult as JSON (same shape as `exec`).",
        name = "eval"
    )]
    Eval {
        /// SKILL expression to evaluate (omit when using --stdin)
        #[arg(default_value = None)]
        code: Option<String>,

        /// Read the SKILL expression from stdin instead of argv
        #[arg(long)]
        stdin: bool,
    },

    /// Search SKILL function names using Cadence SKILL Finder database
    #[command(
        long_about = "Search SKILL function names from the Cadence SKILL Finder database.\n\n\
            The SKILL Finder database contains all public SKILL functions with their\n\
            syntax signatures and one-line descriptions.\n\n\
            Search modes:\n\
            - fuzzy: Case-insensitive substring match (default)\n\
            - prefix: Name starts with query\n\
            - suffix: Name ends with query\n\
            - exact: Exact name match\n\
            - regex: Python regular expression match\n\n\
            Examples:\n\
            virtuoso skill find dbOpen\n\
            virtuoso skill find dbOpen --mode prefix\n\
            virtuoso skill find '^db.*' --mode regex --limit 20\n\n\
            Data source: $IC/doc/finder/SKILL/*.fnd\n\
            Set VB_SKILL_FINDER_DIR to override the default location.",
        name = "find"
    )]
    Find {
        /// Search string or pattern
        query: String,

        /// Search mode
        #[arg(long, short, default_value = "fuzzy")]
        mode: String,

        /// Maximum number of results
        #[arg(long, short, default_value = "50")]
        limit: usize,

        /// Also search the description field, not just the function name.
        /// Useful for "what function does X" questions where you only know
        /// the use case, not the function name.
        #[arg(long)]
        include_desc: bool,
    },

    /// Get detailed More Info documentation for a SKILL function
    #[command(
        long_about = "Get detailed documentation for a specific SKILL function.\n\n\
            Queries the Cadence More Info system via the Virtuoso bridge.\n\
            Returns HTML content with full documentation including Description,\n\
            Arguments, Returns, and Example sections.\n\n\
            Examples:\n\
            virtuoso skill info dbOpenCellView\n\
            virtuoso skill info mfGetOption\n\n\
            Note: Requires Virtuoso connection. The More Info system provides\n\
            detailed HTML documentation indexed by function name.",
        name = "info"
    )]
    Info {
        /// SKILL function name
        func: String,
    },

    /// Sync SKILL Finder database from remote server to local cache
    #[command(
        long_about = "Download the SKILL Finder database from a remote server.\n\n\
            Downloads all .fnd files from the remote `doc/finder/SKILL/` directory\n\
            to the local cache for faster subsequent queries.\n\n\
            Cache location: ~/.cache/virtuoso_bridge/skill_finder/<host>/\n\n\
            Examples:\n\
            virtuoso skill sync\n\
            virtuoso skill sync --host eda-server\n\
            virtuoso skill sync --host eda-server --cshrc /path/to/cshrc",
        name = "sync"
    )]
    Sync {
        /// Remote host (uses VB_HOST or profile remote_host if not specified)
        #[arg(long)]
        host: Option<String>,

        /// Path to Cadence cshrc file (uses VB_CADENCE_CSHRC if not specified)
        #[arg(long)]
        cshrc: Option<String>,

        /// Verbose output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Show cache status for SKILL Finder
    #[command(
        long_about = "Show information about the local SKILL Finder cache.\n\n\
            Displays cache location, number of cached files, and last modified time.\n\n\
            Examples:\n\
            virtuoso skill cache\n\
            virtuoso skill cache --host eda-server",
        name = "cache"
    )]
    Cache {
        /// Remote host (uses VB_HOST or profile remote_host if not specified)
        #[arg(long)]
        host: Option<String>,

        /// Clear the cache
        #[arg(long, short)]
        clear: bool,
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

    /// Set a design variable value
    SetVar {
        #[arg(long)]
        name: String,
        #[arg(long)]
        value: String,
    },

    /// Get a design variable value
    GetVar {
        #[arg(long)]
        name: String,
    },

    /// List all design variables
    ListVars,

    /// Get enabled analyses for a test
    GetAnalyses {
        #[arg(long)]
        session: String,
    },

    /// Enable an analysis type (e.g. ac, dc, tran, noise)
    SetAnalysis {
        #[arg(long)]
        session: String,
        /// Analysis type: ac | dc | tran | noise | ...
        #[arg(long)]
        analysis: String,
        /// Analysis options as JSON string, e.g. '{"start":"1","stop":"10G","dec":"20"}'
        #[arg(long)]
        options: Option<String>,
    },

    /// Add an output expression to a test
    AddOutput {
        /// Output name (e.g. "maxOut")
        #[arg(long)]
        output_name: String,
        /// Test name (e.g. "AC")
        #[arg(long)]
        test_name: String,
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

    /// Export results to CSV via maeExportOutputView
    Export {
        #[arg(long)]
        session: String,
        /// Output CSV file path
        #[arg(long)]
        path: String,
        /// Test name to export (optional; API uses default when omitted)
        #[arg(long)]
        test_name: Option<String>,
        /// History run name, e.g. ExplorerRun.0 (optional; API uses default when omitted)
        #[arg(long)]
        history: Option<String>,
    },

    /// Inspect focused ADE window and return session metadata
    SessionInfo {
        /// Session name for run_dir lookup (optional; omit to skip run_dir)
        #[arg(long)]
        session: Option<String>,
    },

    // --- Result Reading Commands ---
    /// Open a history run for programmatic result access
    OpenResults {
        /// History run name (e.g. "Interactive.1")
        #[arg(long)]
        history: String,
    },

    /// Close the currently open results
    CloseResults,

    /// List all test names that have results in the current history
    ResultTests,

    /// List all output names for a given test in the current history
    ResultOutputs {
        #[arg(long)]
        test_name: String,
    },

    /// Get the value of a specific output
    GetOutputValue {
        #[arg(long)]
        name: String,
        #[arg(long)]
        test_name: String,
        /// Corner name (optional)
        #[arg(long)]
        corner: Option<String>,
    },

    /// Get the spec pass/fail status for an output
    SpecStatus {
        #[arg(long)]
        name: String,
        #[arg(long)]
        test_name: String,
    },

    /// Get simulation messages (errors/warnings) from last run
    SimMessages {
        #[arg(long)]
        session: String,
    },

    /// List available history runs for the current Maestro session
    HistoryList,

    /// Snapshot run artifacts to a local directory (YAML-filtered)
    Snapshot {
        /// Output directory path
        #[arg(long)]
        output: String,
        /// Session name (optional; auto-detects from focused window)
        #[arg(long)]
        session: Option<String>,
        /// History run name (optional; picks newest if omitted)
        #[arg(long)]
        history: Option<String>,
        /// Path to custom filter YAML (optional; uses built-in if omitted)
        #[arg(long)]
        filter: Option<String>,
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

    /// Polish net labels — cosmetic preset, auto-rotation, or repositioning
    PolishLabel {
        /// Net name whose labels to polish
        #[arg(long)]
        net: String,
        /// Preset: "readable" (largest font, center-aligned) or "compact" (smallest font)
        #[arg(long, default_value = "readable")]
        preset: String,
        /// Apply auto-rotation based on wire direction
        #[arg(long)]
        auto_rotate: bool,
        /// Offset in DB units: "small" (+5), "medium" (+10), "large" (+20)
        #[arg(long)]
        offset: Option<String>,
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

    /// Show which session would be auto-selected (dry-run of session discovery)
    Current,

    /// Remove stale session files for daemons that are no longer running
    Cleanup,

    /// Show SKILL and command history for a session
    History {
        /// Session ID to show history for (e.g. eda-meow-34785)
        id: String,
        /// Show only SKILL executions (default: both)
        #[arg(long)]
        skill: bool,
        /// Show only CLI commands (default: both)
        #[arg(long)]
        cmd: bool,
        /// Maximum number of entries to show (0 = all)
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// Start background heartbeat daemon (pings sessions to detect stale Virtuoso)
    Heartbeat {
        /// Heartbeat interval in seconds (default: 30)
        #[arg(long, default_value = "30")]
        interval: u64,
    },
}

#[derive(Subcommand)]
enum TxCmd {
    /// Begin a transaction — captures a snapshot of the currently open cellview
    Begin {
        /// Transaction ID (e.g. "my-design-tx")
        #[arg(long)]
        id: String,

        /// Library name
        #[arg(long)]
        lib: String,

        /// Cell name
        #[arg(long)]
        cell: String,

        /// View name
        #[arg(long, default_value = "schematic")]
        view: String,
    },

    /// Commit the active transaction — discards snapshot
    Commit,

    /// Rollback — restore the cellview to the snapshot state
    Rollback,

    /// Show differences between snapshot and current cellview
    Diff,

    /// Show active transaction status (ID and timestamp)
    Status,
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

#[derive(Subcommand)]
enum RpcCmd {
    /// Call an RPC method by name with JSON params
    ///
    /// Examples:
    ///   vcli rpc call schematic.open_cell_view '{"lib":"myLib","cell":"myCell"}'
    ///   vcli rpc call schematic.list_instances '{}'
    Call {
        /// Method name (e.g. schematic.place)
        #[arg(long)]
        method: String,

        /// JSON params object
        #[arg(long)]
        params: String,
    },

    /// Show all available RPC methods and their signatures
    Schema,
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

fn dispatch_profile(cmd: ProfileCmd) -> error::Result<serde_json::Value> {
    use virtuoso_cli::profile::{BindScope, ProfileResolution};
    match cmd {
        ProfileCmd::Show => {
            let info: ProfileResolution = virtuoso_cli::profile::resolve_profile_info(None);
            Ok(serde_json::json!({
                "profile": info.profile,
                "source": info.source,
                "path": info.path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "resolution_order": [
                    "1. explicit profile= argument / CLI -p/--profile",
                    "2. process env VB_PROFILE",
                    "3. $VIRTUAL_ENV/.vcli-profile (venv binding)",
                    "4. ~/.vcli/.env VB_PROFILE= (user-level default)",
                    "5. None (legacy default)",
                ],
            }))
        }
        ProfileCmd::Bind { name, venv, user, local } => {
            let scope = parse_bind_scope(venv, user, local)?;
            match scope {
                BindScope::Venv => {
                    let path = virtuoso_cli::profile::bind_venv_profile(&name)
                        .map_err(|e| error::VirtuosoError::Config(e.to_string()))?;
                    Ok(serde_json::json!({
                        "action": "bind",
                        "scope": "venv",
                        "profile": name,
                        "path": path.to_string_lossy().to_string(),
                    }))
                }
                BindScope::User => {
                    let path = virtuoso_cli::profile::bind_user_profile(&name)
                        .map_err(|e| error::VirtuosoError::Config(e.to_string()))?;
                    Ok(serde_json::json!({
                        "action": "bind",
                        "scope": "user",
                        "profile": name,
                        "path": path.to_string_lossy().to_string(),
                    }))
                }
                BindScope::Local => {
                    let path = virtuoso_cli::profile::bind_local_profile(&name)
                        .map_err(|e| error::VirtuosoError::Config(e.to_string()))?;
                    Ok(serde_json::json!({
                        "action": "bind",
                        "scope": "local",
                        "profile": name,
                        "path": path.to_string_lossy().to_string(),
                    }))
                }
            }
        }
        ProfileCmd::Clear { venv, user, local } => {
            let scope = parse_bind_scope(venv, user, local)?;
            match scope {
                BindScope::Venv => {
                    let path = virtuoso_cli::profile::clear_venv_profile()
                        .map_err(|e| error::VirtuosoError::Config(e.to_string()))?;
                    Ok(serde_json::json!({
                        "action": "clear",
                        "scope": "venv",
                        "path": path.to_string_lossy().to_string(),
                    }))
                }
                BindScope::User => {
                    virtuoso_cli::profile::clear_user_profile()
                        .map_err(|e| error::VirtuosoError::Config(e.to_string()))?;
                    Ok(serde_json::json!({
                        "action": "clear",
                        "scope": "user",
                    }))
                }
                BindScope::Local => {
                    virtuoso_cli::profile::clear_local_profile()
                        .map_err(|e| error::VirtuosoError::Config(e.to_string()))?;
                    Ok(serde_json::json!({
                        "action": "clear",
                        "scope": "local",
                    }))
                }
            }
        }
    }
}

fn parse_bind_scope(venv: bool, user: bool, local: bool) -> error::Result<virtuoso_cli::profile::BindScope> {
    let set: Vec<&str> = [
        ("venv", venv),
        ("user", user),
        ("local", local),
    ]
    .iter()
    .filter_map(|(n, b)| if *b { Some(*n) } else { None })
    .collect();
    match set.len() {
        0 => Err(error::VirtuosoError::Config(
            "must specify one of --venv, --user, or --local".into(),
        )),
        1 => Ok(match set[0] {
            "venv" => virtuoso_cli::profile::BindScope::Venv,
            "user" => virtuoso_cli::profile::BindScope::User,
            "local" => virtuoso_cli::profile::BindScope::Local,
            _ => unreachable!(),
        }),
        _ => Err(error::VirtuosoError::Config(format!(
            "specify only one of --venv, --user, --local (got: {})",
            set.join(", ")
        ))),
    }
}

fn dispatch_skill(cmd: SkillCmd) -> error::Result<serde_json::Value> {
    match cmd {
        SkillCmd::Exec {
            code,
            timeout,
            readonly,
        } => commands::skill::exec(&code, timeout, readonly),
        SkillCmd::Load { file } => commands::skill::load(&file),
        SkillCmd::Broadcast { code, timeout } => commands::skill::broadcast(&code, timeout),
        SkillCmd::Eval { code, stdin } => commands::skill::eval(code, stdin),
        SkillCmd::Find {
            query,
            mode,
            limit,
            include_desc,
        } => commands::skill::find(&query, &mode, limit, false, include_desc),
        SkillCmd::Info { func } => commands::skill::info(&func),
        SkillCmd::Sync {
            host,
            cshrc,
            verbose,
        } => commands::skill::sync_cache(host.as_deref(), cshrc.as_deref(), verbose),
        SkillCmd::Cache { host, clear } => commands::skill::show_cache(host.as_deref(), clear),
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
            for (key, val) in [
                ("stop", stop),
                ("start", start),
                ("from", from),
                ("to", to),
                ("step", step),
                ("dec", dec),
                ("errpreset", errpreset),
            ] {
                if let Some(v) = val {
                    params.insert(key.into(), v);
                }
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
        MaestroCmd::SetVar { name, value } => commands::maestro::set_var(&name, &value),
        MaestroCmd::GetVar { name } => commands::maestro::get_var(&name),
        MaestroCmd::ListVars => commands::maestro::list_vars(),
        MaestroCmd::GetAnalyses { session } => commands::maestro::get_analyses(&session),
        MaestroCmd::SetAnalysis {
            session,
            analysis,
            options,
        } => commands::maestro::set_analysis(&session, &analysis, options.as_deref()),
        MaestroCmd::AddOutput {
            output_name,
            test_name,
            expr,
        } => commands::maestro::add_output(&output_name, &test_name, &expr),
        MaestroCmd::Run { session } => commands::maestro::run(&session),
        MaestroCmd::Save { session } => commands::maestro::save(&session),
        MaestroCmd::Export {
            session,
            path,
            test_name,
            history,
        } => commands::maestro::export(&session, &path, test_name.as_deref(), history.as_deref()),
        MaestroCmd::SessionInfo { session } => commands::maestro::session_info(session.as_deref()),
        MaestroCmd::OpenResults { history } => commands::maestro::open_results(&history),
        MaestroCmd::CloseResults => commands::maestro::close_results(),
        MaestroCmd::ResultTests => commands::maestro::get_result_tests(),
        MaestroCmd::ResultOutputs { test_name } => {
            commands::maestro::get_result_outputs(&test_name)
        }
        MaestroCmd::GetOutputValue {
            name,
            test_name,
            corner,
        } => commands::maestro::get_output_value(&name, &test_name, corner.as_deref()),
        MaestroCmd::SpecStatus { name, test_name } => {
            commands::maestro::get_spec_status(&name, &test_name)
        }
        MaestroCmd::SimMessages { session } => commands::maestro::get_sim_messages(&session),
        MaestroCmd::HistoryList => commands::maestro::get_history_list(),
        MaestroCmd::Snapshot {
            output,
            session,
            history,
            filter,
        } => commands::maestro::snapshot(
            &output,
            session.as_deref(),
            history.as_deref(),
            filter.as_deref(),
        ),
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
        SchematicCmd::PolishLabel {
            net,
            preset,
            auto_rotate,
            offset,
        } => commands::schematic::polish_label(&net, &preset, auto_rotate, offset.as_deref()),
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

fn dispatch_tx(cmd: TxCmd) -> error::Result<serde_json::Value> {
    match cmd {
        TxCmd::Begin {
            id,
            lib,
            cell,
            view,
        } => commands::transaction::begin(&id, &lib, &cell, &view),
        TxCmd::Commit => commands::transaction::commit(),
        TxCmd::Rollback => commands::transaction::rollback(),
        TxCmd::Diff => commands::transaction::diff(),
        TxCmd::Status => commands::transaction::status(),
    }
}

fn dispatch_rpc(cmd: RpcCmd) -> error::Result<serde_json::Value> {
    match cmd {
        RpcCmd::Call { method, params } => {
            let params: serde_json::Value =
                serde_json::from_str(&params).map_err(crate::error::VirtuosoError::Json)?;
            let client = crate::client::bridge::VirtuosoClient::from_env()?;
            // Read API key from environment if set (VCLI_API_KEY)
            let api_key = std::env::var("VCLI_API_KEY").ok().filter(|k| !k.is_empty());
            let request = crate::rpc::dispatcher::RpcRequest {
                method,
                params,
                api_key,
            };
            crate::rpc::dispatcher::RpcDispatcher::dispatch(&client, request)
        }
        RpcCmd::Schema => {
            let schema = standard_schema();
            Ok(serde_json::to_value(schema).unwrap())
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

    // Initialize auth (reads VCLI_API_KEY from env)
    Auth::init();

    let format = match &cli.format {
        Some(FormatArg::Json) => OutputFormat::Json,
        Some(FormatArg::Table) => OutputFormat::Table,
        None => OutputFormat::resolve(None),
    };

    // Propagate --session so VirtuosoClient::from_env() picks it up.
    //
    // Maestro subcommands define their own --session for Maestro session names
    // (e.g. "fnxSession0"), which are NOT bridge session IDs. Propagating a
    // Maestro session name to VB_SESSION causes VirtuosoClient::from_env() to
    // look for a non-existent bridge session file and fall back to VB_PORT=0,
    // producing ECONNREFUSED. Skip propagation for Maestro commands entirely.
    let session_from_env = std::env::var("VB_SESSION").ok();
    if session_from_env.is_none() && !matches!(&cli.command, Commands::Maestro(_)) {
        if let Some(ref s) = cli.session {
            std::env::set_var("VB_SESSION", s);
        }
    }

    let is_status_cmd = matches!(&cli.command, Commands::Tunnel(TunnelCmd::Status));

    let cli_args: Vec<String> = std::env::args().collect();
    let cli_session = cli.session.clone();

    let result = match cli.command {
        Commands::Init { if_not_exists } => commands::init::run(if_not_exists),
        Commands::Tunnel(cmd) => dispatch_tunnel(cmd, format),
        Commands::Profile(cmd) => dispatch_profile(cmd),
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
            SessionCmd::Current => commands::session::current(),
            SessionCmd::Cleanup => commands::session::cleanup(),
            SessionCmd::History {
                id,
                skill,
                cmd,
                limit,
            } => commands::session::history(&id, skill, cmd, limit),
            SessionCmd::Heartbeat { interval } => {
                let hb = virtuoso_cli::session::SessionHeartbeat::new(interval);
                hb.start();
                tracing::info!("heartbeat daemon started (interval={}s)", interval);
                // Keep the main thread alive — the heartbeat runs in background
                loop {
                    std::thread::park();
                }
            }
        },
        Commands::Tx(cmd) => dispatch_tx(cmd),
        Commands::Rpc(cmd) => dispatch_rpc(cmd),
        Commands::Window(cmd) => dispatch_window(cmd),
        Commands::Schema { all, noun, verb } => {
            let schema = if all || noun.is_none() {
                commands::schema::show(None, None)
            } else {
                commands::schema::show(noun.as_deref(), verb.as_deref())
            };
            print_json(&schema);
            std::process::exit(0);
        }
        Commands::Tui => {
            if let Err(e) = tui::run_tui() {
                eprintln!("TUI error: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        Commands::Mcp(cmd) => {
            if let Err(e) = cmd.dispatch() {
                eprintln!("MCP error: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
    };

    let exit_code = match &result {
        Ok(value) => {
            if value
                .get("dry_run")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                exit_codes::DRY_RUN_OK
            } else {
                exit_codes::SUCCESS
            }
        }
        Err(e) => e.exit_code(),
    };
    history::append_cmd(&cli_args, cli_session.as_deref(), exit_code as i32);

    match result {
        Ok(value) => match format {
            OutputFormat::Json => print_json(&value),
            OutputFormat::Table => {
                if !is_status_cmd {
                    output::print_value(&value, format);
                }
            }
        },
        Err(e) => {
            let cli_error = e.to_cli_error();
            cli_error.print(format);
        }
    }
    std::process::exit(exit_code);
}
