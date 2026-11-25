
/// Um KeyboardCapture de um form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardCapture {
    pub id: ComponentId,
}

impl Handle for KeyboardCapture {
    fn id(&self) -> ComponentId {
        self.id
    }
}

/// Quem disparou um handler tipado (`sender: KeyboardCapture`): só um KeyboardCapture.
impl aylis::prelude::FromSender for KeyboardCapture {
    fn from_sender(ui: &Ui, id: ComponentId) -> Option<Self> {
        (ui.class_name(id) == Some("KeyboardCapture")).then_some(Self { id })
    }
}

impl KeyboardCapture {
    pub fn active(self, ui: &Ui) -> bool {
        handles::read(ui, self.id, "active").and_then(|v| v.to_bool().ok()).unwrap_or_default()
    }
    pub fn set_active(self, ui: &mut Ui, value: bool) {
        handles::write(ui, self.id, "active", value)
    }
    pub fn alt_f4(self, ui: &Ui) -> bool {
        handles::read(ui, self.id, "alt_f4").and_then(|v| v.to_bool().ok()).unwrap_or_default()
    }
    pub fn set_alt_f4(self, ui: &mut Ui, value: bool) {
        handles::write(ui, self.id, "alt_f4", value)
    }
    pub fn on_capture_change(self, ui: &Ui) -> String {
        handles::text(ui, handles::read(ui, self.id, "on_capture_change"))
    }
    pub fn set_on_capture_change(self, ui: &mut Ui, value: impl Into<String>) {
        handles::write(ui, self.id, "on_capture_change", Into::<String>::into(value))
    }
    pub fn available(self, ui: &Ui) -> bool {
        handles::read(ui, self.id, "available").and_then(|v| v.to_bool().ok()).unwrap_or_default()
    }
    pub fn set_available(self, ui: &mut Ui, value: bool) {
        handles::write(ui, self.id, "available", value)
    }
    pub fn is_capturing(self, ui: &mut Ui) -> Result<bool, String> {
        let v = handles::try_call(ui, self.id, "is_capturing", &[])?;
        Ok(v.to_bool().unwrap_or_default())
    }
}
