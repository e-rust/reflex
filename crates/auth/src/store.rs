//! Usuários, regras do master e configuração do host (§11.2, §11.3).
//!
//! [`StoreData`] guarda tudo e aplica as regras; [`SignedFile`] persiste com
//! HMAC (edição feita por fora do Reflex é detectada); [`Storage`] abstrai
//! onde os bytes ficam (arquivo de verdade ou memória nos testes).

use hmac::{Hmac, Mac};
use reflex_proto::msg::{HostConfig, LogEntry, UserInfo};
use reflex_proto::perms::{Level, Perms};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::identity::AccessId;
use crate::invite::InviteState;

/// Quantas linhas de log ficam guardadas.
pub const LOG_LIMIT: usize = 2000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("não permitido: {0}")]
    NotAllowed(&'static str),
    #[error("usuário não encontrado")]
    NotFound,
    #[error("usuário já cadastrado")]
    AlreadyExists,
    #[error("ID de acesso inválido")]
    BadAccessId,
    #[error("dados do Reflex foram alterados por fora (assinatura não confere)")]
    Tampered,
    #[error("dados ilegíveis: {0}")]
    Corrupt(String),
    #[error("E/S: {0}")]
    Io(String),
}

/// Quem está pedindo a alteração.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// Usuário local sem privilégio de administrador.
    LocalUser,
    /// Administrador local (pela interface, via pipe do serviço).
    LocalAdmin,
    /// Master conectado pela API remota.
    RemoteMaster(AccessId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub access_id: AccessId,
    pub name: String,
    pub level: Level,
    pub perms: Perms,
    pub confirm: bool,
    pub last_seen: u64,
}

impl User {
    pub fn new(access_id: AccessId, name: &str, level: Level) -> Self {
        Self { access_id, name: name.to_string(), level, perms: level.default_perms(), confirm: false, last_seen: 0 }
    }

    /// Permissões efetivas: master tem tudo; os outros nunca têm ADMIN.
    pub fn effective_perms(&self) -> Perms {
        if self.level.is_master() {
            Perms::ALL
        } else {
            self.perms.without(Perms::ADMIN)
        }
    }

    pub fn to_info(&self) -> UserInfo {
        UserInfo {
            access_id: self.access_id.to_string(),
            name: self.name.clone(),
            level: self.level,
            perms: self.effective_perms(),
            confirm: self.confirm && !self.level.is_master(),
            last_seen: self.last_seen,
        }
    }

    pub fn from_info(info: &UserInfo) -> Result<Self, StoreError> {
        let access_id = AccessId::parse(&info.access_id).ok_or(StoreError::BadAccessId)?;
        Ok(Self { access_id, name: info.name.clone(), level: info.level, perms: info.perms, confirm: info.confirm, last_seen: info.last_seen })
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StoreData {
    pub users: Vec<User>,
    pub config: HostConfig,
    pub invite: InviteState,
    pub log: Vec<LogEntry>,
}

impl StoreData {
    pub fn has_master(&self) -> bool {
        self.users.iter().any(|u| u.level.is_master())
    }

    pub fn find(&self, id: &AccessId) -> Option<&User> {
        self.users.iter().find(|u| &u.access_id == id)
    }

    fn position(&self, id: &AccessId) -> Option<usize> {
        self.users.iter().position(|u| &u.access_id == id)
    }

    pub fn add_user(&mut self, actor: &Actor, user: User) -> Result<(), StoreError> {
        match actor {
            Actor::LocalUser => return Err(StoreError::NotAllowed("só administrador cadastra usuários")),
            Actor::LocalAdmin if user.level.is_master() && self.has_master() => {
                return Err(StoreError::NotAllowed("a máquina já tem master; só um master adiciona outro"))
            }
            _ => {}
        }
        if self.find(&user.access_id).is_some() {
            return Err(StoreError::AlreadyExists);
        }
        self.users.push(user);
        Ok(())
    }

    pub fn update_user(&mut self, actor: &Actor, user: User) -> Result<(), StoreError> {
        let idx = self.position(&user.access_id).ok_or(StoreError::NotFound)?;
        match actor {
            Actor::LocalUser => return Err(StoreError::NotAllowed("só administrador altera usuários")),
            Actor::LocalAdmin => {
                if self.users[idx].level.is_master() {
                    return Err(StoreError::NotAllowed("master só é alterado por um master conectado"));
                }
                if user.level.is_master() && self.has_master() {
                    return Err(StoreError::NotAllowed("a máquina já tem master; só um master promove outro"));
                }
            }
            Actor::RemoteMaster(_) => {}
        }
        let last_seen = self.users[idx].last_seen;
        self.users[idx] = User { last_seen, ..user };
        Ok(())
    }

    pub fn remove_user(&mut self, actor: &Actor, id: &AccessId) -> Result<User, StoreError> {
        let idx = self.position(id).ok_or(StoreError::NotFound)?;
        match actor {
            Actor::LocalUser => return Err(StoreError::NotAllowed("só administrador remove usuários")),
            Actor::LocalAdmin if self.users[idx].level.is_master() => {
                return Err(StoreError::NotAllowed("master só é removido por um master conectado"))
            }
            _ => {}
        }
        Ok(self.users.remove(idx))
    }

    pub fn set_config(&mut self, actor: &Actor, config: HostConfig) -> Result<(), StoreError> {
        if *actor == Actor::LocalUser {
            return Err(StoreError::NotAllowed("só administrador altera a configuração"));
        }
        self.config = config;
        Ok(())
    }

    pub fn touch(&mut self, id: &AccessId, now: u64) {
        if let Some(i) = self.position(id) {
            self.users[i].last_seen = now;
        }
    }

    pub fn log(&mut self, now: u64, who: &str, text: impl Into<String>) {
        self.log.push(LogEntry { time: now, access_id: who.to_string(), text: text.into() });
        if self.log.len() > LOG_LIMIT {
            let drop = self.log.len() - LOG_LIMIT;
            self.log.drain(..drop);
        }
    }
}

// ---------------------------------------------------------------------------
// Persistência assinada
// ---------------------------------------------------------------------------

/// Onde os bytes ficam.
pub trait Storage {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreError>;
    fn save(&self, bytes: &[u8]) -> Result<(), StoreError>;
}

/// Arquivo com troca atômica (grava `.tmp` e renomeia).
pub struct FileStorage {
    pub path: std::path::PathBuf,
}

impl Storage for FileStorage {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreError> {
        match std::fs::read(&self.path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    fn save(&self, bytes: &[u8]) -> Result<(), StoreError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| StoreError::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| StoreError::Io(e.to_string()))
    }
}

/// Memória (testes).
#[derive(Default)]
pub struct MemStorage(pub std::sync::Mutex<Option<Vec<u8>>>);

impl Storage for MemStorage {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, bytes: &[u8]) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    data: String,
    mac: String,
}

/// [`StoreData`] em JSON + HMAC-SHA256 com o segredo da máquina.
pub struct SignedFile<S: Storage> {
    storage: S,
    secret: Vec<u8>,
}

impl<S: Storage> SignedFile<S> {
    pub fn new(storage: S, secret: Vec<u8>) -> Self {
        Self { storage, secret }
    }

    fn mac(&self, data: &str) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC aceita qualquer chave");
        m.update(data.as_bytes());
        m.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Lê; sem arquivo = estado inicial. Assinatura errada = [`StoreError::Tampered`].
    pub fn load(&self) -> Result<StoreData, StoreError> {
        let Some(bytes) = self.storage.load()? else { return Ok(StoreData::default()) };
        let env: Envelope = serde_json::from_slice(&bytes).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        if self.mac(&env.data) != env.mac {
            return Err(StoreError::Tampered);
        }
        serde_json::from_str(&env.data).map_err(|e| StoreError::Corrupt(e.to_string()))
    }

    pub fn save(&self, data: &StoreData) -> Result<(), StoreError> {
        let json = serde_json::to_string(data).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        let env = Envelope { mac: self.mac(&json), data: json };
        let bytes = serde_json::to_vec_pretty(&env).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        self.storage.save(&bytes)
    }

    /// Lê, altera e grava.
    pub fn update<T>(&self, f: impl FnOnce(&mut StoreData) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let mut data = self.load()?;
        let out = f(&mut data)?;
        self.save(&data)?;
        Ok(out)
    }
}

impl<S: Storage> Storage for &S {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreError> {
        (**self).load()
    }
    fn save(&self, bytes: &[u8]) -> Result<(), StoreError> {
        (**self).save(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> AccessId {
        AccessId::of_cert(&[n])
    }

    fn master(n: u8) -> User {
        User::new(id(n), "master", Level::Master)
    }

    fn user(n: u8) -> User {
        User::new(id(n), "user", Level::FullControl)
    }

    #[test]
    fn local_user_cannot_change_anything() {
        let mut s = StoreData::default();
        assert!(matches!(s.add_user(&Actor::LocalUser, user(1)), Err(StoreError::NotAllowed(_))));
        assert!(s.set_config(&Actor::LocalUser, HostConfig::default()).is_err());
    }

    #[test]
    fn local_admin_adds_first_master_only_while_there_is_none() {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, master(1)).unwrap();
        assert!(s.has_master());
        assert!(matches!(s.add_user(&Actor::LocalAdmin, master(2)), Err(StoreError::NotAllowed(_))));
        // Usuário comum continua liberado pro admin local.
        s.add_user(&Actor::LocalAdmin, user(3)).unwrap();
    }

    #[test]
    fn local_admin_cannot_remove_or_demote_master() {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, master(1)).unwrap();
        assert!(matches!(s.remove_user(&Actor::LocalAdmin, &id(1)), Err(StoreError::NotAllowed(_))));
        let demoted = User { level: Level::View, ..master(1) };
        assert!(matches!(s.update_user(&Actor::LocalAdmin, demoted), Err(StoreError::NotAllowed(_))));
        assert!(s.has_master());
    }

    #[test]
    fn local_admin_cannot_promote_when_master_exists() {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, master(1)).unwrap();
        s.add_user(&Actor::LocalAdmin, user(2)).unwrap();
        let promoted = User { level: Level::Master, ..user(2) };
        assert!(s.update_user(&Actor::LocalAdmin, promoted).is_err());
    }

    #[test]
    fn remote_master_manages_everything_including_other_masters() {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, master(1)).unwrap();
        let m = Actor::RemoteMaster(id(1));
        s.add_user(&m, master(2)).unwrap();
        s.add_user(&m, user(3)).unwrap();
        s.remove_user(&m, &id(2)).unwrap();
        s.update_user(&m, User { level: Level::View, ..user(3) }).unwrap();
        assert_eq!(s.find(&id(3)).unwrap().level, Level::View);
    }

    #[test]
    fn last_master_removing_himself_returns_to_no_master() {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, master(1)).unwrap();
        s.remove_user(&Actor::RemoteMaster(id(1)), &id(1)).unwrap();
        assert!(!s.has_master());
        // Admin local pode cadastrar outro.
        s.add_user(&Actor::LocalAdmin, master(9)).unwrap();
    }

    #[test]
    fn duplicates_and_missing_users() {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, user(1)).unwrap();
        assert_eq!(s.add_user(&Actor::LocalAdmin, user(1)), Err(StoreError::AlreadyExists));
        assert_eq!(s.remove_user(&Actor::LocalAdmin, &id(7)).unwrap_err(), StoreError::NotFound);
    }

    #[test]
    fn effective_perms_never_give_admin_to_non_master() {
        let u = User { perms: Perms::ALL, ..user(1) };
        assert!(!u.effective_perms().contains(Perms::ADMIN));
        let m = User { perms: Perms::VIEW, ..master(2) };
        assert_eq!(m.effective_perms(), Perms::ALL);
    }

    #[test]
    fn update_keeps_last_seen() {
        let mut s = StoreData::default();
        s.add_user(&Actor::LocalAdmin, user(1)).unwrap();
        s.touch(&id(1), 1234);
        s.update_user(&Actor::LocalAdmin, User { name: "novo".into(), ..user(1) }).unwrap();
        assert_eq!(s.find(&id(1)).unwrap().last_seen, 1234);
    }

    #[test]
    fn log_is_capped() {
        let mut s = StoreData::default();
        for i in 0..(LOG_LIMIT + 10) {
            s.log(i as u64, "x", "linha");
        }
        assert_eq!(s.log.len(), LOG_LIMIT);
        assert_eq!(s.log[0].time, 10);
    }

    #[test]
    fn signed_file_roundtrip_and_tamper_detection() {
        let f = SignedFile::new(MemStorage::default(), b"segredo".to_vec());
        assert_eq!(f.load().unwrap(), StoreData::default());
        f.update(|d| d.add_user(&Actor::LocalAdmin, master(1))).unwrap();
        assert!(f.load().unwrap().has_master());

        // Alguém troca o nível no arquivo à mão.
        let raw = String::from_utf8(f.storage.load().unwrap().unwrap()).unwrap();
        let edited = raw.replace("Master", "View");
        f.storage.save(edited.as_bytes()).unwrap();
        assert_eq!(f.load().unwrap_err(), StoreError::Tampered);
    }

    #[test]
    fn signed_file_with_other_secret_is_rejected() {
        let storage = MemStorage::default();
        SignedFile::new(&storage, b"a".to_vec()).save(&StoreData::default()).unwrap();
        assert_eq!(SignedFile::new(&storage, b"b".to_vec()).load().unwrap_err(), StoreError::Tampered);
    }

    #[test]
    fn file_storage_roundtrip() {
        let dir = std::env::temp_dir().join(format!("reflex-store-{}", std::process::id()));
        let f = SignedFile::new(FileStorage { path: dir.join("store.json") }, b"k".to_vec());
        f.update(|d| d.add_user(&Actor::LocalAdmin, user(5))).unwrap();
        assert!(f.load().unwrap().find(&id(5)).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
