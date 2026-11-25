//! A sessão de acesso remoto do GNOME (o Mutter, pelo D-Bus da sessão do
//! usuário): a tela vira um fluxo do PipeWire e o teclado e o mouse entram por
//! ela, sem pedir autorização na tela (docs/linux.md).
//!
//! Uma sessão só por processo ([`shared`]), aberta uma vez e mantida: com a
//! tela bloqueada o GNOME recusa abrir outra, e ela acaba quando o processo
//! que a criou sai do D-Bus.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;

const RD: &str = "org.gnome.Mutter.RemoteDesktop";
const RD_PATH: &str = "/org/gnome/Mutter/RemoteDesktop";
const SC: &str = "org.gnome.Mutter.ScreenCast";
const SC_PATH: &str = "/org/gnome/Mutter/ScreenCast";

/// O cursor vem à parte, como metadado do fluxo (`cursor-mode` = 2).
const CURSOR_METADATA: u32 = 2;

/// Os eixos da roda (`NotifyPointerAxisDiscrete`).
pub const AXIS_VERTICAL: u32 = 0;
pub const AXIS_HORIZONTAL: u32 = 1;

/// O monitor gravado: o nó do PipeWire, o tamanho e onde fica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub node: u32,
    pub name: String,
    pub position: (i32, i32),
    pub size: (u32, u32),
}

/// Uma sessão RemoteDesktop + ScreenCast aberta.
pub struct Session {
    conn: Connection,
    rd: OwnedObjectPath,
    stream_path: OwnedObjectPath,
    pub stream: Stream,
}

fn dbus(e: zbus::Error) -> String {
    format!("D-Bus: {e}")
}

impl Session {
    /// Abre a sessão, grava o monitor principal e espera o nó do PipeWire.
    pub fn open() -> Result<Self, String> {
        let conn = Connection::session().map_err(|e| format!("sem o D-Bus da sessão gráfica ({e})"))?;
        let rd: OwnedObjectPath = call(&conn, RD, RD_PATH, RD, "CreateSession", &()).map_err(|e| {
            if e.contains("inhibited") { "o GNOME recusou a sessão de acesso remoto: a tela está bloqueada".to_string() } else { e }
        })?;
        let id: String = property(&conn, RD, &rd, &format!("{RD}.Session"), "SessionId")?;
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("remote-desktop-session-id", Value::from(id.as_str()));
        let sc: OwnedObjectPath = call(&conn, SC, SC_PATH, SC, "CreateSession", &(opts,))?;
        let mut rec: HashMap<&str, Value> = HashMap::new();
        rec.insert("cursor-mode", Value::from(CURSOR_METADATA));
        let stream_path: OwnedObjectPath = call(&conn, SC, &sc, &format!("{SC}.Session"), "RecordMonitor", &("", rec))?;

        // O nó só existe depois do Start: escuta antes, pra não perder o sinal.
        let rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(format!("{SC}.Stream"))
            .map_err(dbus)?
            .member("PipeWireStreamAdded")
            .map_err(dbus)?
            .path(stream_path.clone())
            .map_err(dbus)?
            .build();
        let signals = MessageIterator::for_match_rule(rule, &conn, Some(4)).map_err(dbus)?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for msg in signals.flatten() {
                if let Ok(node) = msg.body().deserialize::<u32>() {
                    let _ = tx.send(node);
                    return;
                }
            }
        });
        call::<()>(&conn, RD, &rd, &format!("{RD}.Session"), "Start", &())?;
        let node = rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "o Mutter não entregou o fluxo da tela".to_string())?;

        let params: HashMap<String, OwnedValue> = property(&conn, SC, &stream_path, &format!("{SC}.Stream"), "Parameters")?;
        let stream = stream_from(node, &params);
        log::info!("Mutter: sessão {} gravando {} ({}x{} em {:?}), nó {node}", rd.as_str(), stream.name, stream.size.0, stream.size.1, stream.position);
        Ok(Self { conn, rd, stream_path, stream })
    }

    fn notify(&self, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) {
        if let Err(e) = call::<()>(&self.conn, RD, &self.rd, &format!("{RD}.Session"), method, body) {
            log::warn!("Mutter {method}: {e}");
        }
    }

    /// Mouse na posição (pixels do monitor gravado).
    pub fn pointer_motion(&self, x: f64, y: f64) {
        self.notify("NotifyPointerMotionAbsolute", &(self.stream_path.as_str(), x, y));
    }

    /// Botão do mouse pelo código do evdev (`BTN_LEFT` = 0x110...).
    pub fn pointer_button(&self, button: i32, pressed: bool) {
        self.notify("NotifyPointerButton", &(button, pressed));
    }

    /// Dentes da roda (`steps` > 0: pra baixo / pra direita).
    pub fn pointer_axis(&self, axis: u32, steps: i32) {
        self.notify("NotifyPointerAxisDiscrete", &(axis, steps));
    }

    /// Tecla pelo código do evdev (`KEY_A` = 30...).
    pub fn key(&self, keycode: u32, pressed: bool) {
        self.notify("NotifyKeyboardKeycode", &(keycode, pressed));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = call::<()>(&self.conn, RD, &self.rd, &format!("{RD}.Session"), "Stop", &());
    }
}

/// A sessão do processo: abre na 1ª vez e fica (com a tela bloqueada não
/// daria pra abrir outra). Falhou: a próxima chamada tenta de novo.
pub fn shared() -> Result<Arc<Session>, String> {
    static SHARED: Mutex<Option<Arc<Session>>> = Mutex::new(None);
    let mut slot = SHARED.lock().map_err(|_| "sessão do Mutter envenenada".to_string())?;
    if let Some(s) = slot.as_ref() {
        return Ok(s.clone());
    }
    let s = Arc::new(Session::open()?);
    *slot = Some(s.clone());
    Ok(s)
}

/// O monitor do fluxo pelos `Parameters` do Mutter. Pura.
fn stream_from(node: u32, params: &HashMap<String, OwnedValue>) -> Stream {
    let pair = |key: &str| params.get(key).and_then(|v| <(i32, i32)>::try_from(v.try_clone().ok()?).ok());
    let name = params.get("output-name").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_else(|| "monitor".into());
    let (w, h) = pair("size").unwrap_or((0, 0));
    Stream { node, name, position: pair("position").unwrap_or((0, 0)), size: (w.max(0) as u32, h.max(0) as u32) }
}

fn call<R>(conn: &Connection, dest: &str, path: &str, iface: &str, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> Result<R, String>
where
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let path = ObjectPath::try_from(path).map_err(|e| e.to_string())?;
    let reply = conn.call_method(Some(dest), path, Some(iface), method, body).map_err(|e| format!("{method}: {e}"))?;
    reply.body().deserialize::<R>().map_err(|e| format!("{method}: {e}"))
}

fn property<T>(conn: &Connection, dest: &str, path: &str, iface: &str, name: &str) -> Result<T, String>
where
    T: TryFrom<OwnedValue>,
    T::Error: std::fmt::Display,
{
    let v: OwnedValue = call(conn, dest, path, "org.freedesktop.DBus.Properties", "Get", &(iface, name))?;
    T::try_from(v).map_err(|e| format!("{name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_parameters_become_the_monitor() {
        let mut p = HashMap::new();
        p.insert("size".to_string(), OwnedValue::try_from(Value::from((1920i32, 1080i32))).unwrap());
        p.insert("position".to_string(), OwnedValue::try_from(Value::from((0i32, 0i32))).unwrap());
        p.insert("output-name".to_string(), OwnedValue::try_from(Value::from("Virtual-1")).unwrap());
        assert_eq!(stream_from(59, &p), Stream { node: 59, name: "Virtual-1".into(), position: (0, 0), size: (1920, 1080) });
        // Sem os parâmetros: tamanho zero (quem usa confere).
        assert_eq!(stream_from(1, &HashMap::new()).size, (0, 0));
    }
}
