use std::{fs, io, path};
use std::error::Error;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use flate2::bufread::GzDecoder;
use git2::Repository;
use reqwest::{get, Client};
use rrgen::RRgen;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tar::Archive;
use tempfile::tempdir;
use tokio::fs::{copy, create_dir_all, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info};
use tracing::field::debug;
use walkdir::WalkDir;
use zip::ZipArchive;

#[derive(Serialize, Deserialize)]
pub struct Generator {
    #[serde(rename = "apiVersion")]
    api_version: String,

    #[serde(rename = "name")]
    name: String,

    #[serde(rename = "version")]
    version: String,

    #[serde(rename = "description")]
    description: String,

    #[serde(rename = "keywords")]
    keywords: Vec<String>,

    #[serde(rename = "home")]
    home: String,

    #[serde(rename = "sources")]
    sources: Vec<String>,

    #[serde(rename = "dependencies")]
    dependencies: Vec<Dependency>,

    #[serde(rename = "maintainers")]
    maintainers: Vec<Maintainer>,

    #[serde(rename = "icon")]
    icon: String,

    #[serde(rename = "deprecated")]
    deprecated: String,

    #[serde(rename = "annotations")]
    annotations: Annotations,
}

#[derive(Serialize, Deserialize)]
pub struct Annotations {
    #[serde(rename = "example")]
    example: String,
}

#[derive(Serialize, Deserialize)]
pub struct Dependency {
    #[serde(rename = "name")]
    name: String,

    #[serde(rename = "version")]
    version: String,

    #[serde(rename = "repository")]
    repository: String,

    #[serde(rename = "condition")]
    condition: String,

    #[serde(rename = "tags")]
    tags: Vec<String>,

    #[serde(rename = "import-values")]
    import_values: Vec<String>,

    #[serde(rename = "alias")]
    alias: String,
}

#[derive(Serialize, Deserialize)]
pub struct Maintainer {
    #[serde(rename = "name")]
    name: String,

    #[serde(rename = "email")]
    email: String,

    #[serde(rename = "url")]
    url: String,
}


pub async fn install_template(uri: &String, destination: &PathBuf) {
    let source = uri;
    info!("Starting the install process...");
    debug!("Source: {}, Destination: {}", source, destination.display());
    let generator_dir = prepare_generator_source(uri).await.unwrap();
    debug!("generator_dir:{}", generator_dir.display());
    move_to_repo_root(generator_dir, destination).await.unwrap();
}

async fn validate_generator(generator_dir_path: PathBuf) {
    debug!("Starting validation of {}",generator_dir_path.to_str().unwrap());
    debug!("TODO!");
    todo!()
}

async fn prepare_generator_source(uri: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = Path::new(uri);
    if path.is_dir() {
        debug!("Uri is local directory: {:?}", path.display());
        Ok(Path::new(uri).to_path_buf())
    } else {
        let temp_dir = tempdir().unwrap().into_path();
        debug!("Created temporary directory: {:?}", temp_dir);
        if uri.starts_with("https://github.com") {
            if uri.ends_with(".zip") || uri.ends_with(".tar.gz") {
                info!("Detected GitHub directory URL that is not a repo, downloading specific directory...");
                download_and_extract(uri, &temp_dir);
            } else {
                info!("Detected GitHub directory URL that is a repo, cloning repo...");
                clone_git_repo(uri, &temp_dir)?;
            }
            Ok(temp_dir)
        } else if uri.ends_with(".zip") || uri.ends_with(".tar.gz") {
            info!("Detected URL, downloading file...");
            download_and_extract(uri, &temp_dir);
            Ok(temp_dir)
        } else {
            return Err("Unsupported URI format".into());
        }
    }
}

/// Downloads and extracts an archive (ZIP or TAR.GZ) from a URL.
async fn download_and_extract(uri: &str, extract_to: &Path) -> Result<(), Box<dyn std::error::Error>> {

    let response= reqwest::get(uri).await?;

    let file_path = extract_to.join("download.zip");

    let mut file = File::create(&file_path).await.unwrap();
    let content =  response.text().await.unwrap();
    file.write_all(content.as_bytes());
    let file = fs::File::open(file_path).unwrap();

    if uri.ends_with(".zip") {
        let mut zip = ZipArchive::new(file).unwrap();
        zip.extract(extract_to)?;
    } else if uri.ends_with(".tar.gz") {
        let buffered_file = BufReader::new(file);
        let tar_gz = GzDecoder::new(buffered_file);
        let mut archive = Archive::new(tar_gz);
        archive.unpack(extract_to)?;
    } else {
        return Err("Unsupported archive format".into());
    }

    Ok(())
}

/// Clones a Git repository to a temporary directory.
fn clone_git_repo(repo_url: &str, clone_to: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Repository::clone(repo_url, clone_to)?;
    Ok(())
}

/// Copies a local file or folder to the temporary directory.
fn copy_local_path(src: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let src_path = Path::new(src);

    if src_path.is_dir() {
        // Recursively copy the directory
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src_path)? {
            let entry = entry?;
            let entry_path = entry.path();
            let dest_path = dest.join(entry.file_name());

            if entry_path.is_dir() {
                fs::create_dir_all(&dest_path)?;
                copy_local_path(entry_path.to_str().unwrap(), &dest_path)?;
            } else {
                fs::copy(&entry_path, &dest_path)?;
            }
        }
    } else if src_path.is_file() {
        // Copy the file
        fs::copy(src_path, dest.join(src_path.file_name().unwrap()))?;
    } else {
        return Err("Invalid source path".into());
    }

    Ok(())
}

/// Moves the generator folder to the repository root after validation.
async fn move_to_repo_root(temp_dir: PathBuf, repo_root: &PathBuf) -> Result<(), io::Error> {
    let path = temp_dir.clone().join("Generator.yaml");
    debug!("Path: {}", path.display());
    let mut file = File::open(path.clone()).await.unwrap();

    // Read the file contents asynchronously into a String
    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;

    // Deserialize from the string contents
    let generator: Generator = serde_yaml::from_str(&contents)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Failed to deserialize Generator.yaml"))?;

    let generator_dir = Path::new(repo_root).join(generator.name.clone()).join(generator.version.clone());

    info!("Installing generator with name:{}, version:{} to directory {}",generator.name.clone(),generator.version.clone(),generator_dir.display());
    if !generator_dir.exists() {
        create_dir_all(&generator_dir);
    }

    for file in WalkDir::new(temp_dir.clone()).into_iter().filter_map(|file| file.ok()) {
        if file.file_type().is_file() {
            let source = file.clone().into_path();
            let stripped_path = file.path().strip_prefix(temp_dir.clone());
            let destination = generator_dir.clone().join(stripped_path.unwrap());
            fs::create_dir_all(destination.parent().unwrap())?;
            debug!("Copying file {} to {}", source.display(),destination.display());
            copy(source, destination).await.unwrap();
        }
    }
    Ok(())
}

pub async fn generate(rrgen:RRgen, generator_dir_path: PathBuf, config: &Path, output: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let mut config_file = File::open(config).await.unwrap();
    let mut contents = String::new();
    config_file.read_to_string(&mut contents).await.unwrap();
    let json_value: Value = serde_json::from_str(&contents)?;

    for file in WalkDir::new(generator_dir_path.clone()).into_iter().filter_map(|file| file.ok()) {
        if file.file_type().is_file() {
            let source = file.clone().into_path();
            let stripped_path = file.path().strip_prefix(generator_dir_path.clone());
            let destination = output.clone().join(stripped_path.unwrap());
            fs::create_dir_all(destination.parent().unwrap())?;
            debug!("copying file {} to {}", source.display(),destination.display());
            if file.file_name().to_str().unwrap().ends_with(".t") {
                rrgen.generate(file.into_path().to_str().unwrap(), &json_value)?;
            }
            else {
                copy(source, destination).await.unwrap();
            }
        }
    }
    Ok(())
}
