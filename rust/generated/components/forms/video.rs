
/// O form frmVideo: os componentes (resolvidos ao abrir; os que só existem em algumas
/// plataformas são `Option`) e o estado dele (`Data`, no arquivo do form). Cada janela
/// aberta tem a sua; os handlers são métodos dela.
pub struct FrmVideo {
    pub form: Form,
    pub lyVideo: Layout,
    pub fvTela: FrameView,
    pub imgCursor: Image,
    pub tmrVideo: Timer,
    pub kcTeclado: KeyboardCapture,
    pub data: super::video::Data,
}

/// Os componentes do frmVideo sem o estado, para o código fora dos handlers
/// (`FrmVideo::components(ui)`): uma cópia, que se usa à vontade com o `ui`.
#[derive(Clone, Copy, Debug)]
pub struct FrmVideoComponents {
    pub form: Form,
    pub lyVideo: Layout,
    pub fvTela: FrameView,
    pub imgCursor: Image,
    pub tmrVideo: Timer,
    pub kcTeclado: KeyboardCapture,
}

impl std::ops::Deref for FrmVideo {
    type Target = Form;
    fn deref(&self) -> &Form {
        &self.form
    }
}

impl FrmVideo {
    /// Monta a instância de um frmVideo recém-criado (o `register` liga). Um componente que
    /// sumiu do form ou mudou de classe impede o form de abrir, com a lista do que não bate.
    pub fn bind(ui: &mut Ui, root: ComponentId) -> Result<Box<dyn std::any::Any>, String> {
        #[allow(unused_mut)] // um form sem componentes não acrescenta nada
        let mut problems: Vec<String> = Vec::new();
        let instance = Self {
            form: Form { id: root },
            lyVideo: Layout { id: handles::resolve(ui, root, "lyVideo", "Layout", &mut problems) },
            fvTela: FrameView { id: handles::resolve(ui, root, "fvTela", "FrameView", &mut problems) },
            imgCursor: Image { id: handles::resolve(ui, root, "imgCursor", "Image", &mut problems) },
            tmrVideo: Timer { id: handles::resolve(ui, root, "tmrVideo", "Timer", &mut problems) },
            kcTeclado: KeyboardCapture { id: handles::resolve(ui, root, "kcTeclado", "KeyboardCapture", &mut problems) },
            data: Default::default(),
        };
        if !problems.is_empty() {
            return Err(format!("o form frmVideo não bate com o código: {}", problems.join("; ")));
        }
        ui.set_form_components(root, Box::new(FrmVideoComponents {
            form: instance.form,
            lyVideo: instance.lyVideo,
            fvTela: instance.fvTela,
            imgCursor: instance.imgCursor,
            tmrVideo: instance.tmrVideo,
            kcTeclado: instance.kcTeclado,
        }));
        Ok(Box::new(instance))
    }

    /// Os componentes do frmVideo aberto (o primeiro, se houver dois), sem o estado: valem
    /// também enquanto um handler dele roda, quando o `get` dá `None`. É o caminho no
    /// código que um handler do próprio frmVideo chama (no handler mesmo, o `self`).
    pub fn components(ui: &Ui) -> Option<FrmVideoComponents> {
        ui.form_components(ui.find_path("frmVideo")?)
    }

    /// Cria o frmVideo (ou devolve o que já está criado); mostrar é com
    /// `ui.show`.
    pub fn create(ui: &mut Ui) -> Result<ComponentId, aylis::prelude::Error> {
        ui.create_form("frmVideo")
    }

    /// Cria mais um frmVideo dentro de `parent` (uma linha, um cartão: cada um com a sua
    /// instância e os handlers falando com ela) e devolve os componentes dele. O nome
    /// segue livre no form de `parent` (`frmVideo`, `frmVideo2`...).
    pub fn create_in(ui: &mut Ui, parent: ComponentId) -> Result<FrmVideoComponents, aylis::prelude::Error> {
        let root = ui.create_form_instance("frmVideo")?;
        let name = ui.free_name_in(parent, "frmVideo");
        if let Err(e) = ui.set_name(root, &name).and_then(|_| ui.set_parent(root, Some(parent))) {
            ui.free(root);
            return Err(e);
        }
        // Como o componente Frame: sem `fill` próprio, transparente (G05).
        if matches!(ui.get_prop(root, "fill"), Ok(aylis::prelude::PropValue::Null)) {
            let _ = ui.set_prop(root, "fill", aylis::prelude::Color::TRANSPARENT);
        }
        ui.form_components(root).ok_or(aylis::prelude::Error::InvalidId)
    }

    /// Os componentes de uma instância do frmVideo (a raiz dela: o `form.id` de um
    /// [`create_in`](Self::create_in)).
    pub fn components_of(ui: &Ui, root: ComponentId) -> Option<FrmVideoComponents> {
        ui.form_components(root)
    }

    /// O frmVideo aberto (o primeiro, se houver dois), para outro form ler ou mexer nele.
    ///
    /// Dentro de um handler do próprio frmVideo, dá `None`: a instância está emprestada ao
    /// handler que roda. Ali, use o `self` do handler ou, no código chamado por ele,
    /// [`components`](Self::components) (em debug, o `None` por empréstimo avisa no stderr).
    pub fn get(ui: &Ui) -> Option<&FrmVideo> {
        ui.form_instance(ui.find_path("frmVideo")?)
    }

    pub fn get_mut(ui: &mut Ui) -> Option<&mut FrmVideo> {
        let form = ui.find_path("frmVideo")?;
        ui.form_instance_mut(form)
    }

    /// O FrameView deste form com o nome (`None` se não há um FrameView com ele).
    pub fn find_frame_view(&self, name: &str) -> Option<FrameView> {
        match name {
            "fvTela" => Some(self.fvTela),
            _ => None,
        }
    }

    /// O FrameView que disparou o evento (`None` se o `sender` não é um FrameView deste form).
    pub fn sender_as_frame_view(&self, ui: &Ui, sender: ComponentId) -> Option<FrameView> {
        self.find_frame_view(ui.name(sender)?)
    }

    /// O Image deste form com o nome (`None` se não há um Image com ele).
    pub fn find_image(&self, name: &str) -> Option<Image> {
        match name {
            "imgCursor" => Some(self.imgCursor),
            _ => None,
        }
    }

    /// O Image que disparou o evento (`None` se o `sender` não é um Image deste form).
    pub fn sender_as_image(&self, ui: &Ui, sender: ComponentId) -> Option<Image> {
        self.find_image(ui.name(sender)?)
    }

    /// O KeyboardCapture deste form com o nome (`None` se não há um KeyboardCapture com ele).
    pub fn find_keyboard_capture(&self, name: &str) -> Option<KeyboardCapture> {
        match name {
            "kcTeclado" => Some(self.kcTeclado),
            _ => None,
        }
    }

    /// O KeyboardCapture que disparou o evento (`None` se o `sender` não é um KeyboardCapture deste form).
    pub fn sender_as_keyboard_capture(&self, ui: &Ui, sender: ComponentId) -> Option<KeyboardCapture> {
        self.find_keyboard_capture(ui.name(sender)?)
    }

    /// O Layout deste form com o nome (`None` se não há um Layout com ele).
    pub fn find_layout(&self, name: &str) -> Option<Layout> {
        match name {
            "lyVideo" => Some(self.lyVideo),
            _ => None,
        }
    }

    /// O Layout que disparou o evento (`None` se o `sender` não é um Layout deste form).
    pub fn sender_as_layout(&self, ui: &Ui, sender: ComponentId) -> Option<Layout> {
        self.find_layout(ui.name(sender)?)
    }

    /// O Timer deste form com o nome (`None` se não há um Timer com ele).
    pub fn find_timer(&self, name: &str) -> Option<Timer> {
        match name {
            "tmrVideo" => Some(self.tmrVideo),
            _ => None,
        }
    }

    /// O Timer que disparou o evento (`None` se o `sender` não é um Timer deste form).
    pub fn sender_as_timer(&self, ui: &Ui, sender: ComponentId) -> Option<Timer> {
        self.find_timer(ui.name(sender)?)
    }
}
