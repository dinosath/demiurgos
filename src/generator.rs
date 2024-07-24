use crate::{Context, Url};
use std::{fs, path::{Path, PathBuf}, io};
use std::collections::HashMap;
use std::io::{BufReader, Cursor, ErrorKind};
use anyhow::anyhow;
use clap::builder::Str;
use flate2::bufread::GzDecoder;
use futures::stream;
use git2::Repository;
use glob::glob;
use reqwest::{get, Client, Response};
use rrgen::{GenResult, RRgen};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tar::Archive;
use tempfile::tempdir;
use tokio::fs::{copy, create_dir_all, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info};
use tracing::field::debug;
use tracing_subscriber::Layer;
use zip::ZipArchive;
use crate::path_to_json;
use serde::de::DeserializeOwned;
use tracing_subscriber::fmt::format;
use tokio_stream::wrappers::ReadDirStream;
use json_value_merge::Merge;
use tokio_stream::StreamExt;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Generator {
    pub base_path: String,
    pub generator_yaml: GeneratorYaml,
    pub license: Option<String>,
    pub readme: Option<String>,
    pub values: Value,
    pub schema: Option<Value>,
    pub files: Option<Vec<String>>,
    pub entities: Value,
    pub templates: Option<Vec<String>>,
    pub dependencies: Option<Vec<Generator>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GeneratorYaml {
    #[serde(rename = "apiVersion")]
    pub api_version: String,

    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "version")]
    pub version: String,

    #[serde(rename = "description")]
    pub description: Option<String>,

    #[serde(rename = "keywords")]
    pub keywords: Option<Vec<String>>,

    #[serde(rename = "home")]
    pub home: Option<String>,

    #[serde(rename = "sources")]
    pub sources: Option<Vec<String>>,

    #[serde(rename = "dependencies")]
    pub dependencies: Option<Vec<Dependency>>,

    #[serde(rename = "maintainers")]
    pub maintainers: Option<Vec<Maintainer>>,

    #[serde(rename = "icon")]
    pub icon: Option<String>,

    #[serde(rename = "deprecated")]
    pub deprecated: Option<bool>,

    #[serde(rename = "annotations")]
    pub annotations: Option<Annotations>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Annotations {
    #[serde(rename = "example")]
    pub example: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Dependency {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "version")]
    pub version: String,

    #[serde(rename = "url")]
    pub repository: Url,

    #[serde(rename = "condition")]
    pub condition: Option<String>,

    #[serde(rename = "tags")]
    pub tags: Option<Vec<String>>,

    #[serde(rename = "import-values")]
    pub import_values: Option<Vec<String>>,

    #[serde(rename = "alias")]
    pub alias: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Maintainer {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "email")]
    pub email: Option<String>,

    #[serde(rename = "url")]
    pub url: Option<String>,
}

pub async fn install_template(uri: &String, destination: &PathBuf) {
    let source = uri;
    info!("Starting the install process...");
    debug!("Source: {}, Destination: {}", source, destination.display());
    let generator_dir = prepare_generator_source(uri).await.unwrap();
    debug!("generator_dir:{}", generator_dir.display());
    move_to_repo_root(generator_dir, destination).await.unwrap();
}

impl Generator {

    fn key(&self) -> String {
        format!("{}:{}", self.generator_yaml.name, self.generator_yaml.version)
    }

    pub async fn from_url(url: &Url, base_path: &Path) -> Result<Self, io::Error> {
        let url_path = if url.scheme() == "file" {
            let url = url.to_string();
            debug!("url: {}", url);
            let file_path = url.strip_prefix("file://").unwrap().to_string();
            debug!("Using url is filesystem path: {}", file_path);
            let path = Path::new(&file_path);
            if path.is_relative() {
                let path = base_path.join(path);
                path
            } else {
                path.to_path_buf()
            }
        } else if url.scheme() == "http" || url.scheme() == "https" {
            // For http:// or https:// URLs, handle download and return a path to the downloaded file
            let temp_path = download_and_extract_to_temp(url.clone()).await?;
            temp_path
        } else {
            // Unsupported scheme
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unsupported URL scheme"));
        };

        // Process the resolved path by calling from_directory (assuming this method exists)
        Self::from_directory(&url_path).await
    }


    pub async fn from_directory(base_path: &Path) -> Result<Self, io::Error> {
        debug!("Creating generator from directory: {}", base_path.display());
        let generator_yaml: GeneratorYaml = read_yaml_file(base_path, "Generator.yaml")?;
        let license = read_optional_file_as_string(base_path, "LICENSE");
        let readme = read_optional_file_as_string(base_path, "README.md");
        let values= read_yaml_file(base_path, "values.yaml")?;
        let schema = read_optional_json_file(base_path, "values.schema.json");
        let files = read_optional_directory(base_path, "files");
        let templates = read_optional_directory(base_path, "templates");
        let entities = read_optional_entities(base_path, "entities")?;

        let dependencies: Vec<Generator> = match &generator_yaml.dependencies {
            None => { vec![] }
            Some(dependencies) => {
                futures::future::join_all(
                    dependencies.iter().map(|dependency| async {
                        Generator::from_url(&dependency.repository, base_path).await.unwrap()
                    })
                ).await
            }
        };

        Ok(Generator {
            base_path: base_path.to_str().unwrap().to_owned(),
            generator_yaml,
            license,
            readme,
            values,
            schema,
            files,
            entities,
            templates,
            dependencies: Some(dependencies),
        })
    }

    pub fn copy_files(&self, destination_dir: &PathBuf) -> Result<(), io::Error> {
        if !destination_dir.is_dir() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Destination directory is not a directory"));
        }

        if !destination_dir.exists() {
            fs::create_dir_all(destination_dir)?;
            debug!("{} - Creating directory {:?}",self.key(), destination_dir.parent().unwrap());
        }

        if self.files.is_none() {
            debug!("{} - There are no files to copy",self.key());
        }
        else {
            debug!("{} - Copying files to destination {:?}", self.key(), destination_dir);
            let base_path = Path::new(&self.base_path).join("files");
            self.files.clone().unwrap().iter().for_each(|file| {
                let file_path = Path::new(file);
                let destination = construct_destination_path(&base_path, &file_path, destination_dir).unwrap();
                fs::create_dir_all(destination.clone().parent().unwrap()).unwrap();
                fs::copy(&file_path, &destination).unwrap();
            });
        }

        if let Some(dependencies) = &self.dependencies {
            for dependency in dependencies {
                dependency.copy_files(destination_dir)?;
            }
        }

        Ok(())
    }

    pub fn generate_templates(&self, rrgen: &mut RRgen, ctx: &Context) -> Result<(), io::Error> {
        debug!("Generator name:{:?},version:{:?}, base_path {:?}",self.generator_yaml.name, self.generator_yaml.version, self.base_path);
        debug!("Generator name:{:?},version:{:?}, Start generating templates {:?}", self.generator_yaml.name, self.generator_yaml.version, self.templates);


        let mut generator_values= self.values.clone();
        debug!("generator_values: {:?}", serde_json::to_string_pretty(&generator_values));
        debug!("values: {:?}", serde_json::to_string_pretty(&ctx));

        if let Some(values_override) = ctx.values
            .as_object()
            .and_then(|value| value.get(self.generator_yaml.name.as_str())) {
            generator_values.merge(values_override);
        }

        debug!("generator_values: {:?}", serde_json::to_string_pretty(&generator_values));

        let context = serde_json::to_value(ctx).unwrap();

        let mut generator_context = json!({
            "values": self.values,
        });
        generator_context.merge(&context);
        debug!("generator_context: {:?}", serde_json::to_string_pretty(&generator_context));

        if let Some(dependencies) = &self.dependencies {
            for dependency in dependencies {
                debug!("Generating templates for dependency: {:?}", dependency.generator_yaml.name);
                dependency.generate_templates(rrgen, ctx)?;
            }
        }

        debug!("Generator name:{:?},version:{:?}, generator_context {:?}",self.generator_yaml.name, self.generator_yaml.version, serde_json::to_string_pretty(&generator_context));
        if self.templates.is_none() || self.templates.clone().unwrap().is_empty() {
            debug!("There are no templates to generate");
        } else {
            rrgen.add_dir_to_tera(Path::new(&self.base_path).join("templates"));
            let mut templates = self.templates.clone().unwrap();
            templates.sort();
            templates.iter()
                .map(|template| Path::new(template))
                .filter(|template| template.is_file() && !(template.file_name().unwrap().to_str().unwrap().starts_with("_") && template.extension().unwrap().to_str().unwrap().eq("tpl")))
                .for_each(|file_path| {
                    let file_name = file_path.file_name().unwrap().to_str().unwrap();
                    let content = fs::read_to_string(file_path).unwrap();
                    debug!("generating file_path:{:?}, file_name:{:?}",file_path, file_name);
                    rrgen.generate(content.as_str(), &generator_context).unwrap();
                });
        }

        Ok(())
    }

    fn collect_templates(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        let key = format!("{}:{}", self.generator_yaml.name, self.generator_yaml.version);
        if let Some(templates) = self.templates.clone() {
            map.insert(key, templates.clone());
        }

        if let Some(dependencies) = &self.dependencies {
            for dep in dependencies {
                let dep_templates = dep.collect_templates();
                map.extend(dep_templates);
            }
        }

        map
    }

    pub(crate) fn collect_entities(&self) -> Value {
        let mut values = self.entities.clone();
        if let Some(dependencies) = &self.dependencies {
            for dep in dependencies {
                let entities = dep.collect_entities();
                values.merge(&entities);
            }
        }
        values.clone()
    }


    fn read_dir_to_vec(dir_path: impl AsRef<Path>) -> Result<Vec<String>, io::Error> {
        let mut file_names = Vec::new();
        if dir_path.as_ref().exists() {
            for entry in fs::read_dir(dir_path)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    if let Some(file_name) = entry.file_name().to_str() {
                        file_names.push(file_name.to_string());
                    }
                }
            }
        }
        Ok(file_names)
    }
}

/// Downloads and extracts an archive (ZIP or TAR.GZ) from a URL.
async fn download_and_extract_to_temp(url: Url) -> Result<PathBuf, io::Error> {
    let temp_dir = tempdir().unwrap().into_path();
    debug!("Downloading to {:?} to {:?}", url,temp_dir);

    let response = reqwest::get(url.clone()).await.map_err(|e| io::Error::new(ErrorKind::Other, format!("Failed to download file {} due to error: {}", url.to_string(), e)))?;

    if !response.status().is_success() {
        return Err(io::Error::new(ErrorKind::Other, format!("Failed to download file from {}. Status code: {}", url, response.status())));
    }

    let bytes = response.bytes().await.map_err(|e| io::Error::new(ErrorKind::Other, format!("Failed to read response bytes of file downloaded from url {} due to error: {}", url.to_string(), e)))?;

    let cursor = Cursor::new(bytes);
    if url.to_string().ends_with(".zip") || url.to_string().ends_with(".tar.gz") {
        let mut zip = ZipArchive::new(cursor)?;
        zip.extract(&temp_dir)?;
    }

    Ok(temp_dir)
}


fn construct_destination_path(base_path: &Path, file: &Path, destination_dir: &Path) -> Result<PathBuf, io::Error> {
    let base_path = base_path.canonicalize().map_err(|e| {
        eprintln!("Error canonicalizing base_path: {:?}", e);
        e
    })?;

    let file = file.canonicalize().map_err(|e| {
        eprintln!("Error canonicalizing file: {:?}", e);
        e
    })?;

    let destination_file_path = file.strip_prefix(&base_path).map_err(|e| {
        eprintln!("Error stripping prefix from file: {:?}", e);
        io::Error::new(io::ErrorKind::Other, "Strip prefix failed")
    })?;
    let destination = destination_dir.join(destination_file_path);
    Ok(destination)
}

fn read_yaml_file<T: for<'de> Deserialize<'de>>(base_path: &Path, file_name: &str) -> Result<T, io::Error> {
    let file_path = base_path.join(file_name);
    let content = fs::read_to_string(file_path.clone())
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, format!("Error reading file {:?} due to the following error:{:?}:", file_path, e)))?;

    let data: T = serde_yaml::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Cannot deserialize file {:?} due to error:{:?}", file_path, e)))?;
    Ok(data)
}

fn read_optional_file_as_string(base_path: &Path, file_name: &str) -> Option<String> {
    fs::read_to_string(base_path.join(file_name)).ok()
}

fn read_optional_json_file(base_path: &Path, file_name: &str) -> Option<serde_json::Value> {
    let content = fs::read_to_string(base_path.join(file_name)).ok()?;
    serde_json::from_str(&content).ok()
}

fn read_optional_directory(base_path: &Path, dir_name: &str) -> Option<Vec<String>> {
    let dir_path = base_path.join(dir_name);
    if !dir_path.exists() || !dir_path.is_dir() {
        return None;
    }

    let glob_pattern = base_path.join(dir_name).join("**/*");

    let files: Vec<String> = glob(glob_pattern.to_str().unwrap())
        .unwrap()
        .filter_map(|x| {
            match x {
                Ok(path) if path.is_file() => {
                    path.to_str().map(|s| s.to_string())
                }
                Ok(_) => None,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    None
                }
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<String>>();

    if files.is_empty() {
        None
    } else {
        Some(files)
    }
}

fn read_optional_entities(base_path: &Path, dir_name: &str) -> Result<Value, io::Error> {
    let dir_path = base_path.join(dir_name);
    if !dir_path.exists() || !dir_path.is_dir() {
        return Ok(json!({}));
    }

    let files_path = base_path.join(dir_name.clone()).join("**/*.schema.json");
    let glob_pattern = files_path.to_str().ok_or_else(|| {
        io::Error::new(ErrorKind::InvalidInput, "Failed to convert pattern to string")
    })?;


    let schemas: Map<String, Value> =
        glob(glob_pattern)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
            .into_iter()
            .filter_map(|file_path_result| {
                let file_path = file_path_result.ok()?;
                let content = fs::read_to_string(&file_path).ok()?;
            let schema: Value = serde_json::from_str(&content).ok()?;
                let file_stem = file_path.file_stem()?.to_str()?.trim_end_matches(".schema").to_string();
            Some((file_stem, schema))
        }).collect();

    Ok(Value::Object(schemas))
}


fn read_required_directory(base_path: &Path, dir_name: &str) -> Result<Vec<String>, io::Error> {
    let dir_path = base_path.join(dir_name);
    if !dir_path.exists() || !dir_path.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("Directory {} not found", dir_name)));
    }
    let files = fs::read_dir(dir_path)?
        .filter_map(|entry| entry.ok().map(|e| e.path().display().to_string()))
        .collect();
    Ok(files)
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
    let response = reqwest::get(uri).await?;

    let file_path = extract_to.join("download.zip");

    let mut file = File::create(&file_path).await.unwrap();
    let content = response.text().await.unwrap();
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
    let generator: GeneratorYaml = serde_yaml::from_str(&contents)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Failed to deserialize Generator.yaml"))?;

    let generator_dir = Path::new(repo_root).join(generator.name.clone()).join(generator.version.clone());

    info!("Installing generator with name:{}, version:{} to directory {}",generator.name.clone(),generator.version.clone(),generator_dir.display());
    if !generator_dir.exists() {
        create_dir_all(&generator_dir);
    }

    // use glob for installing templates
    // for file in WalkDir::new(temp_dir.clone()).into_iter().filter_map(|file| file.ok()) {
    //     if file.file_type().is_file() {
    //         let source = file.clone().into_path();
    //         let stripped_path = file.path().strip_prefix(temp_dir.clone());
    //         let destination = generator_dir.clone().join(stripped_path.unwrap());
    //         fs::create_dir_all(destination.parent().unwrap())?;
    //         debug!("Copying file {} to {}", source.display(),destination.display());
    //         copy(source, destination).await.unwrap();
    //     }
    // }
    Ok(())
}

pub(crate) fn dereference_config(config: &mut Value, parent_path: &Path) {
    // debug!("dereferencing config:{config}");
    let entities = config.get_mut("entities").unwrap().as_object_mut().unwrap();

    entities.values_mut().into_iter().for_each(|elem| {
        let object = elem.as_object_mut().unwrap();
        if object.contains_key("$ref") && object.get("$ref").unwrap().is_string() && object.len() == 1 {
            let reference = object.get("$ref").unwrap().as_str().unwrap();
            debug!("loading file from reference:{reference}");
            let file_path = parent_path.join(reference);
            if file_path.exists() {
                *elem = path_to_json(&file_path).expect(format!("file {} doesnt exist or is an invalid JSON", file_path.display()).as_str());
            } else {
                error!("File {} does not exist",file_path.display());
            }
        }
    });
}