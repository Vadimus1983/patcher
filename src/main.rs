mod apply;
mod binary_diff;
mod binary_patch;
mod create;
mod patch_format;
mod rolling_hash;
mod util;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "patcher", about = "Binary patch creator and applier")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a patch by comparing old and new directories
    Create {
        /// Path to the old (original) directory
        #[arg(long)]
        old: PathBuf,
        /// Path to the new (updated) directory
        #[arg(long)]
        new: PathBuf,
        /// Output path for the patch file
        #[arg(long, short)]
        output: PathBuf,
    },
    /// Apply a patch to a target directory
    Apply {
        /// Path to the target directory to patch
        #[arg(long)]
        target: PathBuf,
        /// Path to the patch file
        #[arg(long, short)]
        patch: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Create { old, new, output } => {
            println!("Creating patch...");
            println!("  Old: {}", old.display());
            println!("  New: {}", new.display());
            println!("  Output: {}", output.display());

            let start = Instant::now();
            let summary = create::create_patch(&old, &new, &output).await?;
            let elapsed = start.elapsed();

            println!("\nPatch created successfully!");
            println!("  Directories created: {}", summary.dirs_created);
            println!("  Files added: {}", summary.files_added);
            println!("  Files modified: {}", summary.files_modified);
            println!("  Files deleted: {}", summary.files_deleted);
            println!("  Directories deleted: {}", summary.dirs_deleted);
            println!("  Time elapsed: {:.3}s", elapsed.as_secs_f64());
        }
        Commands::Apply { target, patch } => {
            println!("Applying patch...");
            println!("  Target: {}", target.display());
            println!("  Patch: {}", patch.display());

            let start = Instant::now();
            let summary = apply::apply_patch(&target, &patch).await?;
            let elapsed = start.elapsed();

            println!("\nPatch applied successfully!");
            println!("  Directories created: {}", summary.dirs_created);
            println!("  Files added: {}", summary.files_added);
            println!("  Files modified: {}", summary.files_modified);
            println!("  Files deleted: {}", summary.files_deleted);
            println!("  Directories deleted: {}", summary.dirs_deleted);
            println!("  Time elapsed: {:.3}s", elapsed.as_secs_f64());
        }
    }

    Ok(())
}
