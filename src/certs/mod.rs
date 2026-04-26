use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use rcgen::{Certificate, CertificateParams, DistinguishedName, IsCa, KeyPair, DnType, BasicConstraints};
use tracing::{info, debug, error};

pub struct CertificateAuthority {
    root_cert: Certificate,
    cert_cache: Arc<RwLock<HashMap<String, (Vec<u8>, Vec<u8>)>>>, // (cert_der, key_der)
    storage_path: PathBuf,
}

impl CertificateAuthority {
    pub async fn new(storage_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        debug!(path = ?storage_path, "Initializing CA");
        let storage_path = storage_path.to_path_buf();
        let root_key_path = storage_path.join("root.key");
        let root_cert_path = storage_path.join("root.crt");

        if !storage_path.exists() {
            debug!("Creating CA storage directory");
            fs::create_dir_all(&storage_path)?;
        }

        // Bug fix: previously both branches called generate_root_ca, which ignored the loaded
        // key pair and wrote new files on every restart. Now we reconstruct the Certificate
        // from the stored key so domain certs remain valid across restarts.
        let root_cert = if root_key_path.exists() && root_cert_path.exists() {
            debug!("Loading existing root CA from disk");
            let key_pem = fs::read_to_string(&root_key_path)?;
            let key_pair = KeyPair::from_pem(&key_pem)?;
            let mut params = CertificateParams::default();
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.distinguished_name = DistinguishedName::new();
            params.distinguished_name.push(DnType::CommonName, "oproxy Root CA");
            params.distinguished_name.push(DnType::OrganizationName, "oproxy");
            params.key_pair = Some(key_pair);
            Certificate::from_params(params)?
        } else {
            info!("Generating new root CA");
            Self::generate_root_ca(&storage_path)?
        };

        Ok(Self {
            root_cert,
            cert_cache: Arc::new(RwLock::new(HashMap::new())),
            storage_path,
        })
    }

    fn generate_root_ca(storage_path: &Path) -> Result<Certificate, Box<dyn std::error::Error>> {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "oproxy Root CA");
        params.distinguished_name.push(DnType::OrganizationName, "oproxy");

        let cert = Certificate::from_params(params)?;
        
        fs::write(storage_path.join("root.key"), cert.serialize_private_key_pem())?;
        fs::write(storage_path.join("root.crt"), cert.serialize_pem()?)?;

        Ok(cert)
    }

    pub async fn get_certificate_for_domain(&self, domain: &str) -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
        debug!(domain = %domain, "Getting certificate for domain");
        {
            let cache = self.cert_cache.read().await;
            if let Some(pair) = cache.get(domain) {
                debug!(domain = %domain, "Certificate cache hit");
                return Ok(pair.clone());
            }
        }
        debug!(domain = %domain, "Certificate cache miss");

        let mut params = CertificateParams::new(vec![domain.to_string()]);
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, domain);

        let cert = Certificate::from_params(params).map_err(|e| {
            error!(error = %e, "Failed to create certificate params");
            e
        })?;
        let cert_der = cert.serialize_der_with_signer(&self.root_cert).map_err(|e| {
            error!(error = %e, "Failed to sign certificate");
            e
        })?;
        let key_der = cert.serialize_private_key_der();

        {
            let mut cache = self.cert_cache.write().await;
            cache.insert(domain.to_string(), (cert_der.clone(), key_der.clone()));
        }

        Ok((cert_der, key_der))
    }

    pub fn get_root_cert_pem(&self) -> String {
        self.root_cert.serialize_pem().unwrap_or_default()
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
        CertificateAuthority::new(&dir).await.expect("CA creation failed");
        assert!(dir.join("root.key").exists(), "root.key must be written");
        assert!(dir.join("root.crt").exists(), "root.crt must be written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn get_root_cert_pem_returns_valid_pem() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir).await.expect("CA creation failed");
        let pem = ca.get_root_cert_pem();
        assert!(!pem.is_empty());
        assert!(pem.contains("BEGIN CERTIFICATE"), "PEM must contain certificate header");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn domain_cert_is_generated_and_signed() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir).await.expect("CA creation failed");
        let (cert_der, key_der) = ca.get_certificate_for_domain("example.com").await.expect("cert gen failed");
        assert!(!cert_der.is_empty(), "cert DER must not be empty");
        assert!(!key_der.is_empty(), "key DER must not be empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn domain_cert_is_cached_on_second_call() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir).await.expect("CA creation failed");
        let first = ca.get_certificate_for_domain("cache.test").await.expect("first call failed");
        let second = ca.get_certificate_for_domain("cache.test").await.expect("second call failed");
        assert_eq!(first, second, "cached cert must be identical to first-generated cert");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn different_domains_produce_different_certs() {
        let dir = temp_ca_dir();
        let ca = CertificateAuthority::new(&dir).await.expect("CA creation failed");
        let (cert_a, _) = ca.get_certificate_for_domain("foo.test").await.expect("foo cert failed");
        let (cert_b, _) = ca.get_certificate_for_domain("bar.test").await.expect("bar cert failed");
        assert_ne!(cert_a, cert_b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Before the bug fix, CertificateAuthority::new always called generate_root_ca even when
    /// existing files were present, silently overwriting root.key and root.crt on every restart.
    /// After the fix the files must be left untouched on a second construction.
    #[tokio::test]
    async fn loading_existing_ca_does_not_overwrite_key_or_cert_files() {
        let dir = temp_ca_dir();

        // First construction – creates the files
        CertificateAuthority::new(&dir).await.expect("first CA failed");
        let key_after_first = std::fs::read_to_string(dir.join("root.key")).expect("root.key missing");
        let crt_after_first = std::fs::read_to_string(dir.join("root.crt")).expect("root.crt missing");

        // Second construction with existing files present – must NOT overwrite anything
        CertificateAuthority::new(&dir).await.expect("second CA failed");
        let key_after_second = std::fs::read_to_string(dir.join("root.key")).expect("root.key missing after reload");
        let crt_after_second = std::fs::read_to_string(dir.join("root.crt")).expect("root.crt missing after reload");

        assert_eq!(key_after_first, key_after_second, "root.key must not be overwritten on reload");
        assert_eq!(crt_after_first, crt_after_second, "root.crt must not be overwritten on reload");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
