use std::fs;
use std::fs::File;
use std::io::copy;
use std::path::Path;

use clap::Parser;
use gix::Repository;
use reqwest::Url;
use zip::ZipArchive;

/// A fictional versioning CLI
#[derive(Parser, Debug)]
#[command(name= "demiurgos", version="0.0.1", about="CLI application for downloading and running tera and rrgen templates", author = "Konstantinos Athanasiou <dinosath0@gmail.com>")]
struct CliArgs {
    #[arg(short, long)]
    install: String,
}

#[tokio::main]
async fn main() {
    let cli_args = CliArgs::parse();
    let source = &cli_args.install;
    let binding = dirs::config_local_dir().unwrap().join("demiurgos");
    let destination = binding.to_str().unwrap();

    if source.starts_with("http://") || source.starts_with("https://") {
        download_from_url(source, destination).await;
    }else if source.starts_with("https://github.com") && !source.ends_with(".git") {
        download_github_directory(source, destination).await;
    }
    else if source.ends_with(".git") {
        clone_git_repo(source, destination);
    } else {
        copy_from_path(source, destination);
    }
}

fn copy_from_path(source: &str, destination: &str) {
    let source_path = Path::new(source);
    let destination_path = Path::new(destination);

    if source_path.is_file() {
        fs::copy(source_path, destination_path).expect("Failed to copy file");
    } else if source_path.is_dir() {
        fs::create_dir_all(destination_path).expect("Failed to create destination directory");
        for entry in fs::read_dir(source_path).expect("Failed to read source directory") {
            let entry = entry.expect("Failed to get directory entry");
            let entry_path = entry.path();
            let entry_destination = destination_path.join(entry.file_name());
            copy_from_path(entry_path.to_str().unwrap(), entry_destination.to_str().unwrap());
        }
    } else {
        eprintln!("Source path does not exist");
    }
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
    let temp_dir = std::env::temp_dir().join("repo_temp");
    let temp_zip_path = temp_dir.join("repo.zip");

    fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");

    let archive_url = format!("{}/archive/refs/heads/master.zip", repo_url);
    let response = reqwest::get(&archive_url).await.expect("Failed to download archive");
    let mut file = File::create(&temp_zip_path).expect("Failed to create temp zip file");
    let content = response.bytes().await.expect("Failed to read content");
    copy(&mut content.as_ref(), &mut file).expect("Failed to write to temp zip file");

    // Extract the zip file
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
        copy_from_path(specific_dir.to_str().unwrap(), destination);
    } else {
        eprintln!("The specified directory does not exist in the repository");
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
