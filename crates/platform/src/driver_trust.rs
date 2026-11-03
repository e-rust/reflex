//! Confiança temporária no assinante de um pacote de driver.
//!
//! Driver assinado fora do programa da Microsoft (o monitor virtual é
//! assinado pela SignPath Foundation) só instala sem perguntar nada se o
//! certificado de quem assinou estiver em "Editores Confiáveis" da máquina.
//! [`TrustedSigner`] põe lá só o certificado que assinou o `.cat` e o tira
//! ao sair de escopo: a máquina não fica confiando em tudo que a SignPath
//! assinar. O driver, uma vez instalado, continua funcionando.

use std::path::Path;

use windows::Win32::Security::Cryptography::*;

use crate::wide;

const ENCODING: CERT_QUERY_ENCODING_TYPE = CERT_QUERY_ENCODING_TYPE(X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0);

/// Enquanto existir, o assinante do `.cat` está em LocalMachine\TrustedPublisher.
pub struct TrustedSigner {
    store: HCERTSTORE,
    /// Cópia que nós adicionamos (sai no drop); `None` se já era confiável.
    added: Option<*mut CERT_CONTEXT>,
}

impl TrustedSigner {
    pub fn for_catalog(cat: &Path) -> Result<Self, String> {
        let signer = Signer::of_file(cat)?;
        let name = wide("TrustedPublisher");
        let store = unsafe { CertOpenStore(CERT_STORE_PROV_SYSTEM_W, CERT_QUERY_ENCODING_TYPE(0), None, CERT_OPEN_STORE_FLAGS(CERT_SYSTEM_STORE_LOCAL_MACHINE), Some(name.as_ptr() as *const _)) }
            .map_err(|e| format!("abrir Editores Confiáveis: {e}"))?;
        let existing = unsafe { CertFindCertificateInStore(store, ENCODING, 0, CERT_FIND_EXISTING, Some(signer.cert as *const _), None) };
        if !existing.is_null() {
            unsafe {
                let _ = CertFreeCertificateContext(Some(existing));
            }
            return Ok(Self { store, added: None });
        }
        let mut added = std::ptr::null_mut();
        unsafe { CertAddCertificateContextToStore(Some(store), signer.cert, CERT_STORE_ADD_NEW, Some(&mut added)) }.map_err(|e| format!("confiar no assinante: {e}"))?;
        Ok(Self { store, added: Some(added) })
    }
}

impl Drop for TrustedSigner {
    fn drop(&mut self) {
        unsafe {
            if let Some(added) = self.added.take() {
                // Apaga e libera o contexto, mesmo se falhar.
                if CertDeleteCertificateFromStore(added).is_err() {
                    log::warn!("não deu pra tirar o assinante do driver dos Editores Confiáveis");
                }
            }
            let _ = CertCloseStore(Some(self.store), 0);
        }
    }
}

/// O certificado que assinou um arquivo PKCS#7 (o `.cat`), com o que ele prende.
struct Signer {
    store: HCERTSTORE,
    msg: *mut core::ffi::c_void,
    cert: *mut CERT_CONTEXT,
}

impl Signer {
    fn of_file(path: &Path) -> Result<Self, String> {
        let file = wide(&path.to_string_lossy());
        let (mut store, mut msg) = (HCERTSTORE::default(), std::ptr::null_mut());
        unsafe {
            CryptQueryObject(CERT_QUERY_OBJECT_FILE, file.as_ptr() as *const _, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED, CERT_QUERY_FORMAT_FLAG_ALL, 0, None, None, None, Some(&mut store), Some(&mut msg), None)
        }
        .map_err(|e| format!("ler a assinatura de {}: {e}", path.display()))?;
        let mut signer = Self { store, msg, cert: std::ptr::null_mut() };
        if msg.is_null() {
            return Err(format!("{} não é uma mensagem assinada", path.display()));
        }
        // CERT_INFO do assinante (emissor + número de série) → o certificado no store da mensagem.
        let mut size = 0u32;
        unsafe { CryptMsgGetParam(msg, CMSG_SIGNER_CERT_INFO_PARAM, 0, None, &mut size) }.map_err(|e| format!("assinante: {e}"))?;
        // u64: o CERT_INFO tem ponteiros, o buffer precisa do alinhamento deles.
        let mut info = vec![0u64; (size as usize).div_ceil(8)];
        unsafe { CryptMsgGetParam(msg, CMSG_SIGNER_CERT_INFO_PARAM, 0, Some(info.as_mut_ptr() as *mut _), &mut size) }.map_err(|e| format!("assinante: {e}"))?;
        signer.cert = unsafe { CertFindCertificateInStore(store, ENCODING, 0, CERT_FIND_SUBJECT_CERT, Some(info.as_ptr() as *const _), None) };
        if signer.cert.is_null() {
            return Err(format!("{} não tem o certificado de quem assinou", path.display()));
        }
        Ok(signer)
    }
}

impl Drop for Signer {
    fn drop(&mut self) {
        unsafe {
            if !self.cert.is_null() {
                let _ = CertFreeCertificateContext(Some(self.cert));
            }
            let _ = CryptMsgClose(Some(self.msg));
            let _ = CertCloseStore(Some(self.store), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signer_of_the_bundled_driver_catalog() {
        let cat = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/vdd/mttvdd.cat");
        let s = Signer::of_file(&cat).unwrap();
        // Nome do assinante: SignPath Foundation.
        let mut name = [0u16; 128];
        let n = unsafe { CertGetNameStringW(s.cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut name)) };
        assert!(n > 1);
        assert_eq!(crate::from_wide(&name), "SignPath Foundation");
    }

    #[test]
    fn unsigned_file_has_no_signer() {
        let inf = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(Signer::of_file(&inf).is_err());
    }
}
