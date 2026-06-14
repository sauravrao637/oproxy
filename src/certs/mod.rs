use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType, IsCa, Issuer,
    KeyPair,
};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

type CertCache = Arc<RwLock<HashMap<String, (Vec<u8>, Vec<u8>)>>>;
type CertResult<T> = Result<T, Box<dyn std::error::Error>>;
type RootMaterial = (CertificateParams, Certificate, KeyPair);
const MAX_CERT_CACHE_ENTRIES: usize = 1024;

struct RootPaths {
    directory: PathBuf,
    key: PathBuf,
    cert: PathBuf,
}

impl RootPaths {
    fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_path_buf(),
            key: directory.join("root.key"),
            cert: directory.join("root.crt"),
        }
    }

    fn exists(&self) -> bool {
        self.key.exists() && self.cert.exists()
    }
}

pub struct CertificateAuthority {
    root_params: CertificateParams,
    root_cert: Certificate,
    root_key: KeyPair,
    cert_cache: CertCache,
}

impl CertificateAuthority {
    pub async fn new(storage_path: &Path) -> CertResult<Self> {
        debug!(path = ?storage_path, "Initializing CA");
        let paths = RootPaths::new(storage_path);
        let (root_params, root_cert, root_key) = Self::load_or_generate_root(&paths)?;

        Ok(Self {
            root_params,
            root_cert,
            root_key,
            cert_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    fn load_or_generate_root(paths: &RootPaths) -> CertResult<RootMaterial> {
        if !paths.directory.exists() {
            debug!("Creating CA storage directory");
            fs::create_dir_all(&paths.directory)?;
        }
        if paths.exists() {
            debug!("Loading existing root CA from disk");
            Self::load_root_ca(paths)
        } else {
            info!("Generating new root CA");
            Self::generate_root_ca(paths)
        }
    }

    fn load_root_ca(paths: &RootPaths) -> CertResult<RootMaterial> {
        harden_private_key_permissions(&paths.key);
        let key_pair = KeyPair::from_pem(&fs::read_to_string(&paths.key)?)?;
        let params = Self::root_params();
        let cert = params.self_signed(&key_pair)?;
        Ok((params, cert, key_pair))
    }

    fn root_params() -> CertificateParams {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.use_authority_key_identifier_extension = true;
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, "oproxy Root CA");
        params
            .distinguished_name
            .push(DnType::OrganizationName, "oproxy");
        params
    }

    fn generate_root_ca(paths: &RootPaths) -> CertResult<RootMaterial> {
        let params = Self::root_params();
        let key_pair = KeyPair::generate()?;
        let cert = params.self_signed(&key_pair)?;

        write_private_key(&paths.key, &key_pair.serialize_pem())?;
        fs::write(&paths.cert, cert.pem())?;

        Ok((params, cert, key_pair))
    }

    pub async fn get_certificate_for_domain(&self, domain: &str) -> CertResult<(Vec<u8>, Vec<u8>)> {
        debug!(domain = %domain, "Getting certificate for domain");
        if let Some(pair) = self.cached_certificate(domain).await {
            debug!(domain = %domain, "Certificate cache hit");
            return Ok(pair);
        }
        debug!(domain = %domain, "Certificate cache miss");

        let pair = self.issue_domain_certificate(domain)?;
        self.cache_certificate(domain, pair.clone()).await;
        Ok(pair)
    }

    async fn cached_certificate(&self, domain: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        self.cert_cache.read().await.get(domain).cloned()
    }

    fn issue_domain_certificate(&self, domain: &str) -> CertResult<(Vec<u8>, Vec<u8>)> {
        let mut params = CertificateParams::new(vec![domain.to_string()])?;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, domain);

        let cert_key = KeyPair::generate()?;
        let issuer = Issuer::from_params(&self.root_params, &self.root_key);
        let cert = params.signed_by(&cert_key, &issuer).map_err(|e| {
            error!(error = %e, "Failed to create certificate params");
            e
        })?;
        Ok((cert.der().to_vec(), cert_key.serialize_der()))
    }

    async fn cache_certificate(&self, domain: &str, pair: (Vec<u8>, Vec<u8>)) {
        let mut cache = self.cert_cache.write().await;
        if cache.len() >= MAX_CERT_CACHE_ENTRIES
            && let Some(evicted) = cache.keys().next().cloned()
        {
            cache.remove(&evicted);
            debug!(domain = %evicted, "Evicted certificate cache entry");
        }
        cache.insert(domain.to_string(), pair);
    }

    pub fn get_root_cert_pem(&self) -> String {
        self.root_cert.pem()
    }
}

fn write_private_key(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())?;
    harden_private_key_permissions(path);
    Ok(())
}

fn harden_private_key_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            warn!(path = %path.display(), error = %e, "Failed to harden CA private key permissions");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_ca_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oproxy_ca_test_{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn new_ca_creates_key_and_cert_files() {
        let dir = temp_ca_dir();
        CertificateAuthority::new(&dir)
            .await
            .expect("CA creation failed");
        assert!(dir.join("root.key").exists(), "root.key must be written");
        assert!(dir.join("root.crt").exists(), "root.crt must be written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn new_ca_writes_private_key_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_ca_dir();
        CertificateAuthority::new(&dir)
            .await
            .expect("CA creation failed");
        let mode = std::fs::metadata(dir.join("root.key"))
            .expect("root.key metadata missing")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn get_root_cert_pem_returns_valid_pem() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir)
            .await
            .expect("CA creation failed");
        let pem = ca.get_root_cert_pem();
        assert!(!pem.is_empty());
        assert!(
            pem.contains("BEGIN CERTIFICATE"),
            "PEM must contain certificate header"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn domain_cert_is_generated_and_signed() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir)
            .await
            .expect("CA creation failed");
        let (cert_der, key_der) = ca
            .get_certificate_for_domain("example.com")
            .await
            .expect("cert gen failed");
        assert!(!cert_der.is_empty(), "cert DER must not be empty");
        assert!(!key_der.is_empty(), "key DER must not be empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn domain_cert_is_cached_on_second_call() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir)
            .await
            .expect("CA creation failed");
        let first = ca
            .get_certificate_for_domain("cache.test")
            .await
            .expect("first call failed");
        let second = ca
            .get_certificate_for_domain("cache.test")
            .await
            .expect("second call failed");
        assert_eq!(
            first, second,
            "cached cert must be identical to first-generated cert"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn different_domains_produce_different_certs() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir)
            .await
            .expect("CA creation failed");
        let (cert_a, _) = ca
            .get_certificate_for_domain("foo.test")
            .await
            .expect("foo cert failed");
        let (cert_b, _) = ca
            .get_certificate_for_domain("bar.test")
            .await
            .expect("bar cert failed");
        assert_ne!(cert_a, cert_b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Before the bug fix, CertificateAuthority::new always called generate_root_ca even when
    /// existing files were present, silently overwriting root.key and root.crt on every restart.
    /// After the fix the files must be left untouched on a second construction.
    #[tokio::test]
    async fn loading_existing_ca_does_not_overwrite_key_or_cert_files() {
        let dir = temp_ca_dir();

        // First construction creates the files.
        CertificateAuthority::new(&dir)
            .await
            .expect("first CA failed");
        let key_after_first =
            std::fs::read_to_string(dir.join("root.key")).expect("root.key missing");
        let crt_after_first =
            std::fs::read_to_string(dir.join("root.crt")).expect("root.crt missing");

        // Second construction with existing files present must not overwrite anything.
        CertificateAuthority::new(&dir)
            .await
            .expect("second CA failed");
        let key_after_second =
            std::fs::read_to_string(dir.join("root.key")).expect("root.key missing after reload");
        let crt_after_second =
            std::fs::read_to_string(dir.join("root.crt")).expect("root.crt missing after reload");

        assert_eq!(
            key_after_first, key_after_second,
            "root.key must not be overwritten on reload"
        );
        assert_eq!(
            crt_after_first, crt_after_second,
            "root.crt must not be overwritten on reload"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
