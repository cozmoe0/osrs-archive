use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use anyhow::{bail, Result};
use jsonwebtoken::Algorithm::RS256;
use jsonwebtoken::{DecodingKey, Validation};
use serde::de::DeserializeOwned;
use serde_json::Value;

const VERSIONS_URL: &str = "https://jagex.akamaized.net/direct6/<repo>/<repo>.json";
const ALIASES_URL: &str = "https://jagex.akamaized.net/direct6/<repo>/alias.json";
const CATALOG_URL: &str = "https://jagex.akamaized.net/direct6/<repo>/catalog/<id>/catalog.json";

#[derive(Debug, Clone)]
pub struct Config {
    pub repo: String,
    pub build: String,
    pub version: Version,
    pub alias: String,
    pub catalog: Catalog,
    pub metafile: Metafile,
}

#[derive(Debug, Clone, Default)]
pub struct Version {
    pub id: String,
    pub promote_time: u64,
    pub scan_time: u64,
    pub version: String
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub base_url: String,
    pub delta_format: String,
    pub flags: String,
    pub piece_format: String,
    pub piece_type: String,
    pub id: String,
    pub meta_file: String
}

#[derive(Debug, Clone, Default)]
pub struct Metafile {
    pub id: String,
    pub files: Vec<MetafileEntry>,
    pub padding: Vec<MetafilePadding>,
    pub pieces: Vec<String>,
    pub pieces_algorithm: String,
    pub hash_padding: bool,
    pub version: String,
    pub scan_time: u64,
    pub algorithm: String,
}

#[derive(Debug, Clone)]
pub struct MetafileEntry {
    pub attr: u64,
    pub name: String,
    pub size: u64
}

#[derive(Debug, Clone)]
pub struct MetafilePadding {
    pub offset: u64,
    pub size: u64
}

impl Config {
    pub fn new(repo: &str, build: &str) -> Self {
       Self {
            repo: repo.to_string(),
            build: build.to_string(),
            version: Version::default(),
            alias: String::new(),
            catalog: Catalog::default(),
            metafile: Metafile::default(),
        }
    }

    pub async fn load_versions(&mut self) -> Result<&mut Self> {
        let url = &self.parse_url(VERSIONS_URL);
        let json = Self::get_config_json(&url).await?;
        let json = &json["environments"][&self.build];

        let version = Version {
            id: json["id"].as_str().unwrap().to_string(),
            promote_time: json["promoteTime"].as_u64().unwrap(),
            scan_time: json["scanTime"].as_u64().unwrap(),
            version: json["version"].as_str().unwrap().to_string(),
        };
        self.version = version;

        Ok(self)
    }

    pub async fn load_alias(&mut self) -> Result<&mut Self> {
        let url = &self.parse_url(ALIASES_URL);
        let json = Self::get_config_json(&url).await?;

        let alias = json[format!("{}.{}", &self.repo, &self.build).as_str()].as_str().unwrap().to_string();
        self.alias = alias;

        Ok(self)
    }

    pub async fn load_catalog(&mut self) -> Result<&mut Self> {
        let url = &self.parse_url(CATALOG_URL);
        let url = url.replace("<id>", &self.alias);
        let json = Self::get_config_json(&url).await?;
        let config = &json["config"]["remote"];

        let catalog = Catalog {
            base_url: config["baseUrl"].as_str().unwrap().to_string(),
            delta_format: config["deltaFormat"].as_str().unwrap().to_string(),
            flags: config["flags"].as_str().unwrap().to_string(),
            piece_format: config["pieceFormat"].as_str().unwrap().to_string(),
            piece_type: config["type"].as_str().unwrap().to_string(),
            id: json["id"].as_str().unwrap().to_string(),
            meta_file: json["metafile"].as_str().unwrap().to_string(),
        };
        self.catalog = catalog;

        Ok(self)
    }

    pub async fn load_metafile(&mut self) -> Result<&mut Self> {
        let url = self.parse_url(self.catalog.meta_file.as_str());
        let json = Self::get_config_json(&url).await?;

        let mut files = Vec::<MetafileEntry>::new();
        for e in json["files"].as_array().unwrap() {
            let entry = MetafileEntry {
                attr: e["attr"].as_u64().unwrap(),
                name: e["name"].as_str().unwrap().to_string(),
                size: e["size"].as_u64().unwrap(),
            };
            files.push(entry);
        }

        let mut pads = Vec::<MetafilePadding>::new();
        for e in json["pad"].as_array().unwrap() {
            let pad = MetafilePadding {
                offset: e["offset"].as_u64().unwrap(),
                size: e["size"].as_u64().unwrap(),
            };
            pads.push(pad);
        }

        let mut digests = Vec::<String>::new();
        for e in json["pieces"]["digests"].as_array().unwrap() {
            digests.push(e.as_str().unwrap().to_string());
        }

        let metafile = Metafile {
            id: json["id"].as_str().unwrap().to_string(),
            files,
            padding: pads,
            pieces: digests,
            pieces_algorithm: json["pieces"]["algorithm"].as_str().unwrap().to_string(),
            hash_padding: json["pieces"]["hashPadding"].as_bool().unwrap(),
            version: json["version"].as_str().unwrap().to_string(),
            scan_time: json["scanTime"].as_u64().unwrap(),
            algorithm: json["algorithm"].as_str().unwrap().to_string(),
        };
        self.metafile = metafile;

        Ok(self)
    }

    pub async fn load_all(&mut self) -> Result<&mut Self> {
        self.load_versions().await?;
        self.load_alias().await?;
        self.load_catalog().await?;
        self.load_metafile().await?;
        Ok(self)
    }

    async fn get_config_json(url: &str) -> Result<Value>
    {
        let http = reqwest::Client::new();
        let response = http.get(url).send().await?;

        let status = response.status();
        if !status.is_success() {
            bail!("Failed to fetch config, status code: {}", status);
        }

        let raw = response.text().await?;

        let mut validation = Validation::new(RS256);
        validation.insecure_disable_signature_validation();
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.required_spec_claims = HashSet::new();

        let decode_key = DecodingKey::from_secret(&[]);
        let token_data = jsonwebtoken::decode(raw.as_str(), &decode_key, &validation)?;
        let claims = token_data.claims;

        Ok(serde_json::from_value(claims)?)
    }

    fn parse_url(&self, url: &str) -> String {
        url.replace("<repo>", &self.repo)
    }
}