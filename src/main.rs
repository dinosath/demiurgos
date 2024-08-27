mod generator;

use generator::install_template;
use std::fs;
use std::fs::File;
use std::io::copy;
use std::path::Path;

use clap::Parser;
use clap_derive::Subcommand;
use reqwest::Url;
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format;
use uuid::Uuid;
use zip::ZipArchive;

/// A fictional versioning CLI
#[derive(Parser, Debug)]
#[command(version, about="CLI application for downloading and running tera and rrgen templates", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Serialize, Deserialize, Debug)]
struct Template {
    name: String,
    version: String,
    description: String,
    dependencies: Vec<Dependency>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Dependency {
    name: String,
    version: String,
    repository: String,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// install template to local repo
    Install {
        /// uri of the template to install
        url: String
    },
    /// create a new template scaffold
    New {
        /// the name of the new template
        name: String,
    },
    Generate {
        config: String,
        uri: String,
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();


    let cli = Cli::parse();
    let local_repo = dirs::data_local_dir().unwrap().join("demiurgos");
    info!("directory for demiurgos config and data: {:?}!", local_repo);
    let local_repo_generators = local_repo.join("generators");
    info!("directory for installing templates: {:?}!", local_repo_generators);
    match &cli.command {
        Commands::Install { url } => {
            info!("dir to install templates: {:?}!", local_repo_generators);
            install_template(&url, &local_repo_generators).await;
        },
        Commands::New { name } => {
            info!("Creating new template: {name}");
            create_new_template(name);
        },
        Commands::Generate { config, uri } => {
            todo!()
        },

    }
}

/// Function to create the new template package
fn create_new_template(name: &str) {
    // Define the directory structure and file contents
    let package_dir = format!("./{}", name);

    if Path::new(&package_dir).exists() {
        error!("Directory '{}' already exists!", name);
        return;
    }

    // Metadata file content
    let metadata = Template {
        name: name.to_string(),
        version: "0.0.1".to_string(),
        description: "A template for".to_string(),
        dependencies: vec![],
    };

    let metadata_yaml = serde_yaml::to_string(&metadata).unwrap();
    serde_yaml::to_writer(File::create(&format!("{}/template.yaml", package_dir)).unwrap(), &metadata_yaml).unwrap();
    fs::create_dir_all(&package_dir).unwrap();

    // Create the files
    create_file(&format!("{}/template.yaml", package_dir), &metadata_yaml);
    create_file(
        &format!("{}/README.md", package_dir),
        "# Template README\n\nThis is your template's README file.",
    );
    create_file(
        &format!("{}/template.html", package_dir),
        "<!-- Your template content here -->",
    );

    println!("Template package '{}' has been created!", name);
}

/// Helper function to create a file with content
fn create_file(path: &str, content: &str) {
    let mut file = File::create(path).unwrap();
    // file.write_all(content.as_bytes()).unwrap();
    println!("Created file: {}", path);
}