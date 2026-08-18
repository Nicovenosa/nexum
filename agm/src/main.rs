use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agm", about = "Agent Package Manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Project directory (defaults to current directory)
    #[arg(short = 'C', long, default_value = ".", global = true)]
    dir: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate agm.json template
    Init,

    /// Install all dependencies to target tool
    Install {
        /// Target tool (claude)
        #[arg(long, default_value = "claude")]
        tool: String,

        /// Install directly from git URL (e.g., https://github.com/user/repo)
        #[arg(long)]
        git: Option<String>,
    },

    /// Uninstall a package
    Uninstall {
        /// Package name
        package: String,
        /// Target tool
        #[arg(long, default_value = "claude")]
        tool: String,
    },

    /// Check for upgradable packages
    Update,

    /// List all dependencies
    List,

    /// Publish package to registry
    Publish {
        /// Registry URL
        #[arg(long)]
        registry: Option<String>,
    },

    /// Clean orphan packages from store
    Gc,

    /// Update agm itself
    SelfUpdate {
        /// Force reinstall even if already on latest
        #[arg(long)]
        force: bool,
    },
}

fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let dir = cli.dir.map(PathBuf::from);

    let result = match cli.command {
        Commands::Init => agm::commands::init::execute(dir),
        Commands::Install { tool, git } => {
            agm::commands::install::execute(&tool, git.as_deref(), dir)
        }
        Commands::Uninstall { package, tool } => {
            agm::commands::uninstall::execute(&package, &tool, dir)
        }
        Commands::Update => agm::commands::update::execute(dir),
        Commands::List => agm::commands::list::execute(dir),
        Commands::Publish { registry } => agm::commands::publish::execute(registry, dir),
        Commands::Gc => agm::commands::gc::execute(),
        Commands::SelfUpdate { force } => {
            let _ = force;
            eprintln!("{}", agm::commands::self_update::disabled_message());
            std::process::exit(agm::commands::self_update::DISABLED_EXIT_CODE);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
