//! Mensagens de cada canal (§10.2, §10.3, §10.7, §10.8, §11.5).
//!
//! Canais:
//! - **controle** (bidirecional, o 1º stream aberto pelo viewer): [`Control`];
//! - **unidirecionais**: a 1ª mensagem é um [`UniKind`] dizendo o que vem;
//! - **por pedido** (bidirecional, aberto por quem precisa do dado): a 1ª
//!   mensagem é um [`Request`], a resposta é uma sequência de [`Reply`];
//! - **datagramas**: [`Datagram`] (posição do cursor).

use serde::{Deserialize, Serialize};

use crate::proto::codec::{Codec, CodecCap, CodecChoice};
use crate::proto::perms::{Level, Perms};

// ---------------------------------------------------------------------------
// Controle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Control {
    Hello(Hello),
    /// Viewer → host: entrar por convite (1ª mensagem do SPAKE2).
    InviteStart { spake: Vec<u8> },
    /// Host → viewer: 2ª mensagem do SPAKE2.
    InviteReply { spake: Vec<u8> },
    /// Os dois lados: prova de que chegaram à mesma chave.
    InviteConfirm { mac: [u8; 32] },
    /// Host → viewer: aguardando o usuário local clicar Permitir.
    AwaitingConfirmation,
    AuthResult(AuthResult),
    MonitorList(Vec<Monitor>),
    /// Viewer → host, a qualquer momento (troca de codec/monitor/fps sem reconectar).
    SessionConfig(SessionConfig),
    ConfigApplied { codec: Codec, width: u32, height: u32 },
    ConfigRejected { reason: String },
    RequestKeyframe,
    Stats(Stats),
    Ping { t_ns: u64 },
    Pong { t_ns: u64, host_ns: u64 },
    ClipboardOffer(ClipboardOffer),
    /// Ctrl+Alt+Del no host.
    SecureAttention,
    Admin { id: u32, req: AdminRequest },
    AdminReply { id: u32, res: AdminResponse },
    Bye { reason: String },
    /// Host → viewer: medidas do motor (a cada poucos segundos, pra diagnóstico).
    HostReport(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub proto: u16,
    pub app_version: String,
    pub machine_name: String,
    pub caps: Caps,
}

/// Codecs: o host informa `encoders`, o viewer `decoders`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Caps {
    pub encoders: Vec<CodecCap>,
    pub decoders: Vec<CodecCap>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthResult {
    pub ok: bool,
    pub reason: String,
    pub session_id: u64,
    pub level: Option<Level>,
    pub perms: Perms,
}

impl AuthResult {
    pub fn denied(reason: impl Into<String>) -> Self {
        Self { ok: false, reason: reason.into(), session_id: 0, level: None, perms: Perms::NONE }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub virtual_display: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub monitor: u32,
    pub codec: CodecChoice,
    pub max_fps: u8,
    pub bitrate_kbps: u32,
    /// 0 = sem limite.
    pub max_width: u32,
    pub max_height: u32,
    pub refine: Refine,
    pub chroma: Chroma,
    pub remote_size: RemoteSize,
    /// Com [`RemoteSize::MatchWindow`]: o tamanho que a tela remota deve ter.
    pub desired: Option<DesiredDisplay>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            monitor: 0,
            codec: CodecChoice::Auto,
            max_fps: 60,
            bitrate_kbps: 8000,
            max_width: 0,
            max_height: 0,
            refine: Refine::default(),
            chroma: Chroma::default(),
            remote_size: RemoteSize::default(),
            desired: None,
        }
    }
}

/// Resolução de cor do vídeo. O H.264 comum (4:2:0) guarda a cor com metade
/// da resolução: em foto não se nota, em texto colorido e bordas finas de
/// ícone fica um leve borrão. 4:4:4 guarda a cor inteira — só com o x264
/// (CPU) no host, que só existe se a build do FFmpeg o tiver (é GPL; a do
/// Reflex não tem); sem ele, fica 4:2:0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Chroma {
    #[default]
    Yuv420,
    Yuv444,
}

impl Chroma {
    pub const ALL: [Chroma; 2] = [Chroma::Yuv420, Chroma::Yuv444];

    pub fn label(self) -> &'static str {
        match self {
            Chroma::Yuv420 => "Normal (4:2:0, mais leve)",
            Chroma::Yuv444 => "Completa (4:4:4, texto nítido)",
        }
    }
}

/// Quem manda na resolução da tela remota. `MatchWindow` é como o RDP: a
/// máquina remota passa a desenhar no tamanho (pixels reais) e na escala do
/// Windows da janela do viewer — nada é ampliado, ícone e texto ficam nítidos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RemoteSize {
    #[default]
    Keep,
    MatchWindow,
}

impl RemoteSize {
    pub const ALL: [RemoteSize; 2] = [RemoteSize::Keep, RemoteSize::MatchWindow];

    pub fn label(self) -> &'static str {
        match self {
            RemoteSize::Keep => "A da máquina remota",
            RemoteSize::MatchWindow => "Igual à janela (como o RDP)",
        }
    }
}

/// Tela que o viewer quer: área da janela em pixels reais e a escala do
/// Windows de lá (100, 125, 150…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredDisplay {
    pub width: u32,
    pub height: u32,
    pub scale_pct: u16,
}

/// Nitidez com a tela parada. O vídeo é comprimido com perda e cada quadro
/// tem uma cota de banda: o que muda sai borrado e melhora nos quadros
/// seguintes — mas, com a tela parada, não há quadro seguinte. Refinando, o
/// host manda mais alguns quadros da MESMA imagem logo depois que a tela
/// para (o encoder aproveita a cota pra melhorar o que ficou borrado) e dá
/// mais folga ao quadro inicial. `Off` é o comportamento original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Refine {
    Off,
    Light,
    #[default]
    Normal,
    Max,
}

/// O que um nível de [`Refine`] faz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefinePlan {
    /// Espera depois do último quadro com mudança antes de refinar.
    pub idle_ms: u32,
    /// Quantos quadros de refino por parada da tela.
    pub frames: u32,
    /// Folga de banda do encoder, em quadros (1 = cada quadro só a sua cota).
    pub vbv_frames: u32,
}

impl Refine {
    pub const ALL: [Refine; 4] = [Refine::Off, Refine::Light, Refine::Normal, Refine::Max];

    pub fn label(self) -> &'static str {
        match self {
            Refine::Off => "Desligado",
            Refine::Light => "Leve",
            Refine::Normal => "Padrão",
            Refine::Max => "Máxima",
        }
    }

    pub fn plan(self) -> RefinePlan {
        match self {
            Refine::Off => RefinePlan { idle_ms: 0, frames: 0, vbv_frames: 1 },
            Refine::Light => RefinePlan { idle_ms: 300, frames: 3, vbv_frames: 2 },
            Refine::Normal => RefinePlan { idle_ms: 150, frames: 6, vbv_frames: 4 },
            Refine::Max => RefinePlan { idle_ms: 100, frames: 12, vbv_frames: 8 },
        }
    }

    /// Pelo nome curto (linha de comando): off, leve, padrao, max. Pura.
    pub fn parse(text: &str) -> Option<Refine> {
        match text.to_ascii_lowercase().as_str() {
            "off" | "desligado" | "0" => Some(Refine::Off),
            "leve" | "light" | "1" => Some(Refine::Light),
            "padrao" | "padrão" | "normal" | "2" => Some(Refine::Normal),
            "max" | "maxima" | "máxima" | "3" => Some(Refine::Max),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Stats {
    pub rtt_us: u32,
    pub decode_us: u32,
    pub frames_dropped: u32,
    pub kbps: u32,
}

// ---------------------------------------------------------------------------
// Streams unidirecionais
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UniKind {
    /// Host → viewer: [`VideoFrame`]s.
    Video,
    /// Viewer → host: [`Input`]s.
    Input,
    /// Host → viewer: [`CursorShape`]s.
    Cursor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoFrame {
    pub seq: u64,
    /// Relógio do host (QPC em ns) no momento da captura.
    pub capture_ns: u64,
    pub key: bool,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Coordenadas normalizadas 0..=65535 no monitor da sessão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    /// Estado completo (posição + botões), não delta: um evento perdido é
    /// corrigido pelo próximo.
    Mouse { x: u16, y: u16, buttons: MouseButtons },
    Wheel { dx: i16, dy: i16 },
    Key { scancode: u16, extended: bool, down: bool },
    /// Viewer perdeu o foco: soltar tudo que estiver pressionado.
    ReleaseAll,
    /// Estado das travas (Caps, Num e Scroll Lock) do viewer: o host aperta o
    /// que precisar pra ficar igual. As teclas de trava não vão como `Key`.
    Locks { caps: bool, num: bool, scroll: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MouseButtons(pub u8);

impl MouseButtons {
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const MIDDLE: u8 = 4;
    pub const X1: u8 = 8;
    pub const X2: u8 = 16;

    pub fn is_down(self, b: u8) -> bool {
        self.0 & b != 0
    }
}

/// Forma do cursor já convertida pra BGRA com alfa (o host converte os
/// formatos monocromáticos do Windows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorShape {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub bgra: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Datagramas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Datagram {
    /// Canto de cima à esquerda da forma, em pixels do monitor remoto (pode
    /// ser negativo: cursor saindo pela borda esquerda ou de cima).
    CursorPos { x: i16, y: i16, visible: bool, shape_id: u32 },
}

impl Datagram {
    /// Um datagrama já é uma mensagem inteira: vai só o corpo, sem o prefixo
    /// de tamanho dos streams. (Até a versão 3 o host mandava COM prefixo e o
    /// viewer lia sem: toda posição de cursor era descartada.)
    pub fn to_bytes(&self) -> Vec<u8> {
        crate::proto::frame::encode_body(self).unwrap_or_default()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        crate::proto::frame::decode(bytes).ok()
    }

    /// Posição do cursor, limitada ao que cabe no datagrama.
    pub fn cursor(x: i32, y: i32, visible: bool, shape_id: u32) -> Self {
        let fit = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        Datagram::CursorPos { x: fit(x), y: fit(y), visible, shape_id }
    }
}

// ---------------------------------------------------------------------------
// Clipboard e arquivos (streams por pedido)
// ---------------------------------------------------------------------------

/// Uma cópia: todos os formatos que o programa de origem pôs no clipboard
/// (texto, HTML, RTF, imagem, tabelas do Excel, formatos próprios…), com o
/// conteúdo junto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardOffer {
    /// Muda a cada cópia; pedidos de arquivos com `seq` velho recebem
    /// [`StreamError::Gone`].
    pub seq: u64,
    pub items: Vec<ClipItem>,
    /// Arquivos/pastas copiados (nomes do 1º nível). O conteúdo não vem junto:
    /// quem cola pede a árvore e os blocos por streams ([`Request`]).
    pub files: Vec<String>,
}

/// Um formato do clipboard e os bytes dele.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipItem {
    pub format: ClipFormatId,
    pub data: Vec<u8>,
}

/// Formato do clipboard. Os registrados por programas ("HTML Format",
/// "Rich Text Format", "Biff12"…) ganham um número diferente em cada máquina:
/// viajam pelo nome. Os padrão do Windows (`CF_UNICODETEXT`=13…), pelo número.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipFormatId {
    Standard(u32),
    Named(String),
}

/// 1ª mensagem de um stream por pedido.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    ClipboardText { seq: u64 },
    FileTree { seq: u64 },
    FileRead { seq: u64, index: u32, offset: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Text(String),
    Tree(Vec<FileEntry>),
    FileHeader { size: u64 },
    Chunk(Vec<u8>),
    Done,
    Error(StreamError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Caminho relativo com `\` (como o Explorer espera no FILEDESCRIPTOR).
    pub rel_path: String,
    pub is_dir: bool,
    pub size: u64,
    /// FILETIME (100 ns desde 1601).
    pub modified: u64,
    pub attributes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamError {
    /// O clipboard da origem mudou depois da oferta.
    Gone,
    NotAllowed,
    Io(String),
}

/// Tamanho dos blocos de arquivo.
pub const FILE_CHUNK: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Administração (§11.5)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AdminRequest {
    ListUsers,
    AddUser(UserInfo),
    UpdateUser(UserInfo),
    RemoveUser { access_id: String },
    GetConfig,
    SetConfig(HostConfig),
    ListSessions,
    Disconnect { session_id: u64 },
    GetLog { since: u64 },
    SetSharing { enabled: bool, level: Level },
    GetInvite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AdminResponse {
    Ok,
    Users(Vec<UserInfo>),
    Config(HostConfig),
    Sessions(Vec<SessionInfo>),
    Log(Vec<LogEntry>),
    Invite(Option<InviteInfo>),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInfo {
    pub access_id: String,
    pub name: String,
    pub level: Level,
    pub perms: Perms,
    /// Exige que alguém na máquina clique Permitir (ignorado pro master).
    pub confirm: bool,
    /// Unix (s); 0 = nunca.
    pub last_seen: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostConfig {
    pub port: u16,
    pub allowed_codecs: Vec<Codec>,
    pub virtual_display: VirtualDisplayMode,
    pub max_fps: u8,
    pub max_bitrate_kbps: u32,
    pub allow_clipboard: bool,
    pub allow_files: bool,
    pub allow_invites: bool,
    /// Convidado só entra depois de alguém clicar Permitir na máquina.
    pub confirm_invites: bool,
    /// Recusa todos menos o master até este instante (Unix s); 0 = aceita.
    pub paused_until: u64,
    pub notify_sessions: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            port: crate::proto::DEFAULT_PORT,
            allowed_codecs: vec![Codec::H264, Codec::Hevc, Codec::Av1],
            virtual_display: VirtualDisplayMode::Auto,
            max_fps: 60,
            max_bitrate_kbps: 20_000,
            allow_clipboard: true,
            allow_files: true,
            allow_invites: true,
            confirm_invites: true,
            paused_until: 0,
            notify_sessions: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualDisplayMode {
    /// Liga só quando não há monitor físico.
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: u64,
    pub who: String,
    pub access_id: String,
    pub level: Level,
    pub remote_addr: String,
    pub started: u64,
    pub via_invite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub time: u64,
    pub access_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteInfo {
    pub code: String,
    pub level: Level,
    /// Unix (s) da próxima troca de código.
    pub next_rotation: u64,
}

// ---------------------------------------------------------------------------
// Pipe de configuração: a aba "Esta máquina" ↔ o serviço (§5.3)
// ---------------------------------------------------------------------------

/// Pedido da interface local pro serviço, pelo pipe nomeado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConfigRequest {
    /// Estado geral: impressão digital, se há master, código de convite atual.
    Status,
    /// Uma operação de administração (usuários, config, convite, sessões).
    Admin(AdminRequest),
}

/// Resposta do serviço.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConfigResponse {
    Status(MachineStatus),
    Admin(AdminResponse),
    /// A ação precisa de administrador e o cliente do pipe não é.
    NeedsAdmin,
}

/// Visão geral da máquina controlada, pra aba "Esta máquina".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineStatus {
    /// Impressão digital do certificado do host (o ID a conferir no viewer).
    pub fingerprint: String,
    pub machine_name: String,
    pub port: u16,
    pub has_master: bool,
    pub user_count: u32,
    /// Convite ativo agora (código + validade), se o compartilhar está ligado.
    pub invite: Option<InviteInfo>,
    /// IPs locais pra informar no outro lado.
    pub addresses: Vec<String>,
}
