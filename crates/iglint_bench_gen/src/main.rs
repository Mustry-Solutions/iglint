//! `iglint-bench-gen`: writes a synthetic Ignition 8.3 repository (projects
//! plus gateway configuration) with a given number of Perspective views.
//! Output is a pure function of `--views`, `--seed` and `--defects`.

mod expr;
mod gson;
mod names;
mod project;
mod rng;
mod script;
mod view;
mod world;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Size {
    Small,
    Medium,
    Large,
    Xl,
}

impl Size {
    fn views(self) -> usize {
        match self {
            Size::Small => 50,
            Size::Medium => 500,
            Size::Large => 5_000,
            Size::Xl => 20_000,
        }
    }
}

/// Generate a synthetic Ignition 8.3 corpus for benchmarks.
#[derive(Debug, Parser)]
#[command(name = "iglint-bench-gen")]
struct Args {
    /// Output directory (must not exist, or be empty, unless --force).
    #[arg(long, short)]
    out: PathBuf,
    /// Preset size: small (50 views), medium (500), large (5,000), xl (20,000).
    #[arg(long, value_enum, conflicts_with = "views")]
    size: Option<Size>,
    /// Number of Perspective views.
    #[arg(long)]
    views: Option<usize>,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Probability that a defect opportunity becomes a seeded defect.
    #[arg(long, default_value_t = 0.05)]
    defects: f64,
    /// Replace an existing, non-empty output directory.
    #[arg(long)]
    force: bool,
}

fn write(root: &Path, rel: &str, text: &str) -> Result<()> {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let views = args.views.or(args.size.map(Size::views)).unwrap_or(50);
    if !(0.0..=1.0).contains(&args.defects) {
        bail!("--defects must be between 0 and 1");
    }
    if args.out.exists() && std::fs::read_dir(&args.out)?.next().is_some() {
        if !args.force {
            bail!("{} is not empty (use --force)", args.out.display());
        }
        std::fs::remove_dir_all(&args.out)?;
    }
    std::fs::create_dir_all(&args.out)?;
    let w = world::World::new(world::Config {
        views,
        seed: args.seed,
        defects: args.defects,
    });
    let root = args.out.as_path();

    let stats = w
        .views
        .par_iter()
        .map(|spec| -> Result<view::Stats> {
            let (text, stats) = view::generate(&w, spec);
            let dir = format!("projects/{}/{}/views/{}", project::HMI, project::PERSPECTIVE, spec.path);
            write(root, &format!("{dir}/view.json"), &text)?;
            write(root, &format!("{dir}/resource.json"), &project::view_manifest())?;
            Ok(stats)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut total = view::Stats::default();
    for s in &stats {
        total.add(s);
    }

    let lib_lines = (0..w.modules.len())
        .into_par_iter()
        .map(|i| -> Result<usize> {
            let (files, lines) = project::library_module(&w, i);
            for f in files {
                write(root, &f.path, &f.text)?;
            }
            Ok(lines)
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .sum::<usize>();

    let mut other = project::projects();
    other.extend(project::named_queries(&w));
    other.extend(project::style_classes(&w));
    other.extend(project::hmi_resources(&w));
    other.extend(project::gateway(&w));
    for f in &other {
        write(root, &f.path, &f.text)?;
    }

    let roles = project::roles(&w);
    println!("views: {} ({:?})", total.views, roles);
    println!("view bytes: {} (mean {})", total.bytes, total.bytes / total.views.max(1));
    println!(
        "components: {} (mean {:.1}/view), bindings: {}, transforms: {}",
        total.components,
        total.components as f64 / total.views.max(1) as f64,
        total.bindings,
        total.transforms
    );
    println!("view scripts: {} ({} lines)", total.scripts, total.script_lines);
    println!("library modules: {} ({} lines)", w.modules.len(), lib_lines);
    println!(
        "named queries: {}, style classes: {}, tag lines: {}",
        w.queries.len(),
        w.style_classes.len(),
        w.lines
    );
    let defects: Vec<String> = total.defects.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!("seeded defects: {}", defects.join(", "));
    Ok(())
}
