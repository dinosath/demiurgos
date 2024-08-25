use std::fs;
use std::fs::File;
use std::io::copy;
use std::path::Path;

use clap::Parser;
use gix::Repository;
use reqwest::Url;
use semver::Version;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format;
use uuid::Uuid;
use zip::ZipArchive;

/// A fictional versioning CLI
#[derive(Parser, Debug)]
#[command(name= "demiurgos", version="0.0.1", about="CLI application for downloading and running tera and rrgen templates", author = "Konstantinos Athanasiou <dinosath0@gmail.com>")]
struct CliArgs {
    #[arg(short, long)]
    install: String,

    #[arg(short, long)]
    name: String,

    #[arg(short, long)]
    version: String
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli_args = CliArgs::parse();
    let source = &cli_args.install;
    let binding = dirs::data_local_dir().unwrap().join("demiurgos");
    let destination = binding.to_str().unwrap();

    info!("Starting the install process...");
    debug!("Source: {}, Destination: {}", source, destination);


    if source.starts_with("https://github.com") && source.contains("/tree/") {
        info!("Detected GitHub directory URL that is not a repo, downloading specific directory...");
        download_github_directory(source, destination).await;
    } else if source.starts_with("http://") || source.starts_with("https://") {
        info!("Detected URL, downloading file...");
        download_from_url(source, destination).await;
    } else if source.ends_with(".git") {
        info!("Detected Git repository URL, cloning repository...");
        clone_git_repo(source, destination);
    } else {
        info!("Detected local file or directory path, copying...");
        copy_from_path(source, destination);
    }
    info!("Installation process completed.");
}

fn copy_from_path(source: &str, destination: &str) {
    let source_path = Path::new(source);
    let destination_path = Path::new(destination);

    if source_path.is_file() {
        debug!("Copying file from {} to {}", source, destination);
        fs::copy(source_path, destination_path).expect("Failed to copy file");
    } else if source_path.is_dir() {
        debug!("Copying directory from {} to {}", source, destination);
        fs::create_dir_all(destination_path).expect("Failed to create destination directory");
        for entry in fs::read_dir(source_path).expect("Failed to read source directory") {
            let entry = entry.expect("Failed to get directory entry");
            let entry_path = entry.path();
            let entry_destination = destination_path.join(entry.file_name());
            copy_from_path(entry_path.to_str().unwrap(), entry_destination.to_str().unwrap());
        }
    } else {
        info!("Source path does not exist");
    }
}

fn get_existing_versions(repo_path: &Path) -> Vec<Version> {
    let mut versions = Vec::new();

    if let Ok(entries) = fs::read_dir(repo_path) {
        for entry in entries {
            if let Ok(entry) = entry {
                if let Ok(file_name) = entry.file_name().into_string() {
                    if let Ok(version) = Version::parse(&file_name) {
                        versions.push(version);
                    }
                }
            }
        }
    }
    versions
}


async fn download_from_url(source: &str, destination: &str) {
    let url = Url::parse(source).expect("Invalid URL");
    let response = reqwest::get(url).await.expect("Failed to download file");
    let mut file = File::create(destination).expect("Failed to create destination file");
    let content = response.bytes().await.expect("Failed to read content");
    copy(&mut content.as_ref(), &mut file).expect("Failed to write content");
}

async fn download_github_directory(source: &str, destination: &str) {
    let (repo_url, path_in_repo) = extract_github_info(source);
    let path= format!("repo_temp-{}",Uuid::new_v4());
    let temp_dir = std::env::temp_dir().join(path);
    let temp_zip_path = temp_dir.join("repo.zip");

    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let archive_url = format!("{}/archive/refs/heads/master.zip", repo_url);
    debug!("Downloading GitHub archive from {}", archive_url);
    let response = reqwest::get(&archive_url).await.expect("Failed to download archive");
    let mut file = File::create(&temp_zip_path).expect("Failed to create temp zip file");
    let content = response.bytes().await.expect("Failed to read content");

    debug!("Extracting from zip archive {:?}", temp_zip_path);
    copy(&mut content.as_ref(), &mut file).expect("Failed to write to temp zip file");
    let mut zip_file = File::open(&temp_zip_path).expect("Failed to open temp zip file");
    let mut archive = ZipArchive::new(zip_file).expect("Failed to read zip archive");

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).expect("Failed to access file in zip");
        let outpath = temp_dir.join(file.sanitized_name());

        if (&*file.name()).ends_with('/') {
            fs::create_dir_all(&outpath).expect("Failed to create directory");
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(p).expect("Failed to create directory");
                }
            }
            let mut outfile = File::create(&outpath).expect("Failed to create file");
            copy(&mut file, &mut outfile).expect("Failed to copy file");
        }
    }

    // Move the specific directory to the destination
    let specific_dir = temp_dir.join(format!("{}-master", repo_url.split('/').last().unwrap())).join(path_in_repo);

    if specific_dir.exists() && specific_dir.is_dir() {
        debug!("Copying extracted directory to destination");
        copy_from_path(specific_dir.to_str().unwrap(), destination);
    } else {
        error!("The specified directory does not exist in the repository");
    }

    // Clean up
    fs::remove_dir_all(temp_dir).expect("Failed to remove temp directory");
}

fn extract_github_info(github_url: &str) -> (String, String) {
    let parts: Vec<&str> = github_url.split("/tree/master/").collect();
    let repo_url = parts[0].replace("https://github.com", "https://github.com");
    let path_in_repo = if parts.len() == 2 { parts[1] } else { "" };
    (repo_url, path_in_repo.to_string())
}


fn clone_git_repo(source: &str, destination: &str) {
    //TODO
    // clone git repo to destination
}
