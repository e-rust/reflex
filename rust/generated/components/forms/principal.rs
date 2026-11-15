
/// O form frmPrincipal: os componentes (resolvidos ao abrir; os que só existem em algumas
/// plataformas são `Option`) e o estado dele (`Data`, no arquivo do form). Cada janela
/// aberta tem a sua; os handlers são métodos dela.
pub struct FrmPrincipal {
    pub form: Form,
    pub tabPrincipal: TabControl,
    pub tabConectar: TabItem,
    pub lblTituloConectar: Label,
    pub lblEndereco: Label,
    pub lyEndereco: Layout,
    pub btnConectar: Button,
    pub edtEndereco: Edit,
    pub lblStatus: Label,
    pub lblMeuId: Label,
    pub lyMeuId: Layout,
    pub btnCopiarId: Button,
    pub edtMeuId: Edit,
    pub lblOpcoes: Label,
    pub lyNitidez: Layout,
    pub lblNitidez: Label,
    pub cmbNitidez: ComboBox,
    pub lyEscala: Layout,
    pub lblEscala: Label,
    pub cmbEscala: ComboBox,
    pub lyResolucao: Layout,
    pub lblResolucao: Label,
    pub cmbResolucao: ComboBox,
    pub lyCor: Layout,
    pub lblCor: Label,
    pub cmbCor: ComboBox,
    pub lyCursor: Layout,
    pub lblCursor: Label,
    pub cmbCursor: ComboBox,
    pub lyClipboard: Layout,
    pub lblClipboard: Label,
    pub cmbClipboard: ComboBox,
    pub tabMaquina: TabItem,
    pub lyTituloMaquina: Layout,
    pub btnAtualizar: Button,
    pub lblTituloMaquina: Label,
    pub lblServico: Label,
    pub lblIdMaquina: Label,
    pub lblEnderecos: Label,
    pub lblUsuarios: Label,
    pub lvUsuarios: ListView,
    pub lyCompartilhar: Layout,
    pub btnCompartilhar: Button,
    pub lblConvite: Label,
    pub data: super::principal::Data,
}

/// Os componentes do frmPrincipal sem o estado, para o código fora dos handlers
/// (`FrmPrincipal::components(ui)`): uma cópia, que se usa à vontade com o `ui`.
#[derive(Clone, Copy, Debug)]
pub struct FrmPrincipalComponents {
    pub form: Form,
    pub tabPrincipal: TabControl,
    pub tabConectar: TabItem,
    pub lblTituloConectar: Label,
    pub lblEndereco: Label,
    pub lyEndereco: Layout,
    pub btnConectar: Button,
    pub edtEndereco: Edit,
    pub lblStatus: Label,
    pub lblMeuId: Label,
    pub lyMeuId: Layout,
    pub btnCopiarId: Button,
    pub edtMeuId: Edit,
    pub lblOpcoes: Label,
    pub lyNitidez: Layout,
    pub lblNitidez: Label,
    pub cmbNitidez: ComboBox,
    pub lyEscala: Layout,
    pub lblEscala: Label,
    pub cmbEscala: ComboBox,
    pub lyResolucao: Layout,
    pub lblResolucao: Label,
    pub cmbResolucao: ComboBox,
    pub lyCor: Layout,
    pub lblCor: Label,
    pub cmbCor: ComboBox,
    pub lyCursor: Layout,
    pub lblCursor: Label,
    pub cmbCursor: ComboBox,
    pub lyClipboard: Layout,
    pub lblClipboard: Label,
    pub cmbClipboard: ComboBox,
    pub tabMaquina: TabItem,
    pub lyTituloMaquina: Layout,
    pub btnAtualizar: Button,
    pub lblTituloMaquina: Label,
    pub lblServico: Label,
    pub lblIdMaquina: Label,
    pub lblEnderecos: Label,
    pub lblUsuarios: Label,
    pub lvUsuarios: ListView,
    pub lyCompartilhar: Layout,
    pub btnCompartilhar: Button,
    pub lblConvite: Label,
}

impl std::ops::Deref for FrmPrincipal {
    type Target = Form;
    fn deref(&self) -> &Form {
        &self.form
    }
}

impl FrmPrincipal {
    /// Monta a instância de um frmPrincipal recém-criado (o `register` liga). Um componente que
    /// sumiu do form ou mudou de classe impede o form de abrir, com a lista do que não bate.
    pub fn bind(ui: &mut Ui, root: ComponentId) -> Result<Box<dyn std::any::Any>, String> {
        let mut problems: Vec<String> = Vec::new();
        let instance = Self {
            form: Form { id: root },
            tabPrincipal: TabControl { id: handles::resolve(ui, root, "tabPrincipal", "TabControl", &mut problems) },
            tabConectar: TabItem { id: handles::resolve(ui, root, "tabConectar", "TabItem", &mut problems) },
            lblTituloConectar: Label { id: handles::resolve(ui, root, "lblTituloConectar", "Label", &mut problems) },
            lblEndereco: Label { id: handles::resolve(ui, root, "lblEndereco", "Label", &mut problems) },
            lyEndereco: Layout { id: handles::resolve(ui, root, "lyEndereco", "Layout", &mut problems) },
            btnConectar: Button { id: handles::resolve(ui, root, "btnConectar", "Button", &mut problems) },
            edtEndereco: Edit { id: handles::resolve(ui, root, "edtEndereco", "Edit", &mut problems) },
            lblStatus: Label { id: handles::resolve(ui, root, "lblStatus", "Label", &mut problems) },
            lblMeuId: Label { id: handles::resolve(ui, root, "lblMeuId", "Label", &mut problems) },
            lyMeuId: Layout { id: handles::resolve(ui, root, "lyMeuId", "Layout", &mut problems) },
            btnCopiarId: Button { id: handles::resolve(ui, root, "btnCopiarId", "Button", &mut problems) },
            edtMeuId: Edit { id: handles::resolve(ui, root, "edtMeuId", "Edit", &mut problems) },
            lblOpcoes: Label { id: handles::resolve(ui, root, "lblOpcoes", "Label", &mut problems) },
            lyNitidez: Layout { id: handles::resolve(ui, root, "lyNitidez", "Layout", &mut problems) },
            lblNitidez: Label { id: handles::resolve(ui, root, "lblNitidez", "Label", &mut problems) },
            cmbNitidez: ComboBox { id: handles::resolve(ui, root, "cmbNitidez", "ComboBox", &mut problems) },
            lyEscala: Layout { id: handles::resolve(ui, root, "lyEscala", "Layout", &mut problems) },
            lblEscala: Label { id: handles::resolve(ui, root, "lblEscala", "Label", &mut problems) },
            cmbEscala: ComboBox { id: handles::resolve(ui, root, "cmbEscala", "ComboBox", &mut problems) },
            lyResolucao: Layout { id: handles::resolve(ui, root, "lyResolucao", "Layout", &mut problems) },
            lblResolucao: Label { id: handles::resolve(ui, root, "lblResolucao", "Label", &mut problems) },
            cmbResolucao: ComboBox { id: handles::resolve(ui, root, "cmbResolucao", "ComboBox", &mut problems) },
            lyCor: Layout { id: handles::resolve(ui, root, "lyCor", "Layout", &mut problems) },
            lblCor: Label { id: handles::resolve(ui, root, "lblCor", "Label", &mut problems) },
            cmbCor: ComboBox { id: handles::resolve(ui, root, "cmbCor", "ComboBox", &mut problems) },
            lyCursor: Layout { id: handles::resolve(ui, root, "lyCursor", "Layout", &mut problems) },
            lblCursor: Label { id: handles::resolve(ui, root, "lblCursor", "Label", &mut problems) },
            cmbCursor: ComboBox { id: handles::resolve(ui, root, "cmbCursor", "ComboBox", &mut problems) },
            lyClipboard: Layout { id: handles::resolve(ui, root, "lyClipboard", "Layout", &mut problems) },
            lblClipboard: Label { id: handles::resolve(ui, root, "lblClipboard", "Label", &mut problems) },
            cmbClipboard: ComboBox { id: handles::resolve(ui, root, "cmbClipboard", "ComboBox", &mut problems) },
            tabMaquina: TabItem { id: handles::resolve(ui, root, "tabMaquina", "TabItem", &mut problems) },
            lyTituloMaquina: Layout { id: handles::resolve(ui, root, "lyTituloMaquina", "Layout", &mut problems) },
            btnAtualizar: Button { id: handles::resolve(ui, root, "btnAtualizar", "Button", &mut problems) },
            lblTituloMaquina: Label { id: handles::resolve(ui, root, "lblTituloMaquina", "Label", &mut problems) },
            lblServico: Label { id: handles::resolve(ui, root, "lblServico", "Label", &mut problems) },
            lblIdMaquina: Label { id: handles::resolve(ui, root, "lblIdMaquina", "Label", &mut problems) },
            lblEnderecos: Label { id: handles::resolve(ui, root, "lblEnderecos", "Label", &mut problems) },
            lblUsuarios: Label { id: handles::resolve(ui, root, "lblUsuarios", "Label", &mut problems) },
            lvUsuarios: ListView { id: handles::resolve(ui, root, "lvUsuarios", "ListView", &mut problems) },
            lyCompartilhar: Layout { id: handles::resolve(ui, root, "lyCompartilhar", "Layout", &mut problems) },
            btnCompartilhar: Button { id: handles::resolve(ui, root, "btnCompartilhar", "Button", &mut problems) },
            lblConvite: Label { id: handles::resolve(ui, root, "lblConvite", "Label", &mut problems) },
            data: Default::default(),
        };
        if !problems.is_empty() {
            return Err(format!("o form frmPrincipal não bate com o código: {}", problems.join("; ")));
        }
        ui.set_form_components(root, Box::new(FrmPrincipalComponents {
            form: instance.form,
            tabPrincipal: instance.tabPrincipal,
            tabConectar: instance.tabConectar,
            lblTituloConectar: instance.lblTituloConectar,
            lblEndereco: instance.lblEndereco,
            lyEndereco: instance.lyEndereco,
            btnConectar: instance.btnConectar,
            edtEndereco: instance.edtEndereco,
            lblStatus: instance.lblStatus,
            lblMeuId: instance.lblMeuId,
            lyMeuId: instance.lyMeuId,
            btnCopiarId: instance.btnCopiarId,
            edtMeuId: instance.edtMeuId,
            lblOpcoes: instance.lblOpcoes,
            lyNitidez: instance.lyNitidez,
            lblNitidez: instance.lblNitidez,
            cmbNitidez: instance.cmbNitidez,
            lyEscala: instance.lyEscala,
            lblEscala: instance.lblEscala,
            cmbEscala: instance.cmbEscala,
            lyResolucao: instance.lyResolucao,
            lblResolucao: instance.lblResolucao,
            cmbResolucao: instance.cmbResolucao,
            lyCor: instance.lyCor,
            lblCor: instance.lblCor,
            cmbCor: instance.cmbCor,
            lyCursor: instance.lyCursor,
            lblCursor: instance.lblCursor,
            cmbCursor: instance.cmbCursor,
            lyClipboard: instance.lyClipboard,
            lblClipboard: instance.lblClipboard,
            cmbClipboard: instance.cmbClipboard,
            tabMaquina: instance.tabMaquina,
            lyTituloMaquina: instance.lyTituloMaquina,
            btnAtualizar: instance.btnAtualizar,
            lblTituloMaquina: instance.lblTituloMaquina,
            lblServico: instance.lblServico,
            lblIdMaquina: instance.lblIdMaquina,
            lblEnderecos: instance.lblEnderecos,
            lblUsuarios: instance.lblUsuarios,
            lvUsuarios: instance.lvUsuarios,
            lyCompartilhar: instance.lyCompartilhar,
            btnCompartilhar: instance.btnCompartilhar,
            lblConvite: instance.lblConvite,
        }));
        Ok(Box::new(instance))
    }

    /// Os componentes do frmPrincipal aberto (o primeiro, se houver dois), sem o estado: valem
    /// também enquanto um handler dele roda, quando o `get` dá `None`.
    pub fn components(ui: &Ui) -> Option<FrmPrincipalComponents> {
        ui.form_components(ui.find_path("frmPrincipal")?)
    }

    /// Cria o frmPrincipal (ou devolve o que já está criado); mostrar é com
    /// `ui.show`.
    pub fn create(ui: &mut Ui) -> Result<ComponentId, aylis::prelude::Error> {
        ui.create_form("frmPrincipal")
    }

    /// Cria mais um frmPrincipal dentro de `parent` (uma linha, um cartão: cada um com a sua
    /// instância e os handlers falando com ela) e devolve os componentes dele. O nome
    /// segue livre no form de `parent` (`frmPrincipal`, `frmPrincipal2`...).
    pub fn create_in(ui: &mut Ui, parent: ComponentId) -> Result<FrmPrincipalComponents, aylis::prelude::Error> {
        let root = ui.create_form_instance("frmPrincipal")?;
        let name = ui.free_name_in(parent, "frmPrincipal");
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

    /// Os componentes de uma instância do frmPrincipal (a raiz dela: o `form.id` de um
    /// [`create_in`](Self::create_in)).
    pub fn components_of(ui: &Ui, root: ComponentId) -> Option<FrmPrincipalComponents> {
        ui.form_components(root)
    }

    /// O frmPrincipal aberto (o primeiro, se houver dois), para outro form ler ou mexer nele.
    pub fn get(ui: &Ui) -> Option<&FrmPrincipal> {
        ui.form_instance(ui.find_path("frmPrincipal")?)
    }

    pub fn get_mut(ui: &mut Ui) -> Option<&mut FrmPrincipal> {
        let form = ui.find_path("frmPrincipal")?;
        ui.form_instance_mut(form)
    }

    /// O Button deste form com o nome (`None` se não há um Button com ele).
    pub fn find_button(&self, name: &str) -> Option<Button> {
        match name {
            "btnConectar" => Some(self.btnConectar),
            "btnCopiarId" => Some(self.btnCopiarId),
            "btnAtualizar" => Some(self.btnAtualizar),
            "btnCompartilhar" => Some(self.btnCompartilhar),
            _ => None,
        }
    }

    /// O Button que disparou o evento (`None` se o `sender` não é um Button deste form).
    pub fn sender_as_button(&self, ui: &Ui, sender: ComponentId) -> Option<Button> {
        self.find_button(ui.name(sender)?)
    }

    /// O ComboBox deste form com o nome (`None` se não há um ComboBox com ele).
    pub fn find_combo_box(&self, name: &str) -> Option<ComboBox> {
        match name {
            "cmbNitidez" => Some(self.cmbNitidez),
            "cmbEscala" => Some(self.cmbEscala),
            "cmbResolucao" => Some(self.cmbResolucao),
            "cmbCor" => Some(self.cmbCor),
            "cmbCursor" => Some(self.cmbCursor),
            "cmbClipboard" => Some(self.cmbClipboard),
            _ => None,
        }
    }

    /// O ComboBox que disparou o evento (`None` se o `sender` não é um ComboBox deste form).
    pub fn sender_as_combo_box(&self, ui: &Ui, sender: ComponentId) -> Option<ComboBox> {
        self.find_combo_box(ui.name(sender)?)
    }

    /// O Edit deste form com o nome (`None` se não há um Edit com ele).
    pub fn find_edit(&self, name: &str) -> Option<Edit> {
        match name {
            "edtEndereco" => Some(self.edtEndereco),
            "edtMeuId" => Some(self.edtMeuId),
            _ => None,
        }
    }

    /// O Edit que disparou o evento (`None` se o `sender` não é um Edit deste form).
    pub fn sender_as_edit(&self, ui: &Ui, sender: ComponentId) -> Option<Edit> {
        self.find_edit(ui.name(sender)?)
    }

    /// O Label deste form com o nome (`None` se não há um Label com ele).
    pub fn find_label(&self, name: &str) -> Option<Label> {
        match name {
            "lblTituloConectar" => Some(self.lblTituloConectar),
            "lblEndereco" => Some(self.lblEndereco),
            "lblStatus" => Some(self.lblStatus),
            "lblMeuId" => Some(self.lblMeuId),
            "lblOpcoes" => Some(self.lblOpcoes),
            "lblNitidez" => Some(self.lblNitidez),
            "lblEscala" => Some(self.lblEscala),
            "lblResolucao" => Some(self.lblResolucao),
            "lblCor" => Some(self.lblCor),
            "lblCursor" => Some(self.lblCursor),
            "lblClipboard" => Some(self.lblClipboard),
            "lblTituloMaquina" => Some(self.lblTituloMaquina),
            "lblServico" => Some(self.lblServico),
            "lblIdMaquina" => Some(self.lblIdMaquina),
            "lblEnderecos" => Some(self.lblEnderecos),
            "lblUsuarios" => Some(self.lblUsuarios),
            "lblConvite" => Some(self.lblConvite),
            _ => None,
        }
    }

    /// O Label que disparou o evento (`None` se o `sender` não é um Label deste form).
    pub fn sender_as_label(&self, ui: &Ui, sender: ComponentId) -> Option<Label> {
        self.find_label(ui.name(sender)?)
    }

    /// O Layout deste form com o nome (`None` se não há um Layout com ele).
    pub fn find_layout(&self, name: &str) -> Option<Layout> {
        match name {
            "lyEndereco" => Some(self.lyEndereco),
            "lyMeuId" => Some(self.lyMeuId),
            "lyNitidez" => Some(self.lyNitidez),
            "lyEscala" => Some(self.lyEscala),
            "lyResolucao" => Some(self.lyResolucao),
            "lyCor" => Some(self.lyCor),
            "lyCursor" => Some(self.lyCursor),
            "lyClipboard" => Some(self.lyClipboard),
            "lyTituloMaquina" => Some(self.lyTituloMaquina),
            "lyCompartilhar" => Some(self.lyCompartilhar),
            _ => None,
        }
    }

    /// O Layout que disparou o evento (`None` se o `sender` não é um Layout deste form).
    pub fn sender_as_layout(&self, ui: &Ui, sender: ComponentId) -> Option<Layout> {
        self.find_layout(ui.name(sender)?)
    }

    /// O ListView deste form com o nome (`None` se não há um ListView com ele).
    pub fn find_list_view(&self, name: &str) -> Option<ListView> {
        match name {
            "lvUsuarios" => Some(self.lvUsuarios),
            _ => None,
        }
    }

    /// O ListView que disparou o evento (`None` se o `sender` não é um ListView deste form).
    pub fn sender_as_list_view(&self, ui: &Ui, sender: ComponentId) -> Option<ListView> {
        self.find_list_view(ui.name(sender)?)
    }

    /// O TabControl deste form com o nome (`None` se não há um TabControl com ele).
    pub fn find_tab_control(&self, name: &str) -> Option<TabControl> {
        match name {
            "tabPrincipal" => Some(self.tabPrincipal),
            _ => None,
        }
    }

    /// O TabControl que disparou o evento (`None` se o `sender` não é um TabControl deste form).
    pub fn sender_as_tab_control(&self, ui: &Ui, sender: ComponentId) -> Option<TabControl> {
        self.find_tab_control(ui.name(sender)?)
    }

    /// O TabItem deste form com o nome (`None` se não há um TabItem com ele).
    pub fn find_tab_item(&self, name: &str) -> Option<TabItem> {
        match name {
            "tabConectar" => Some(self.tabConectar),
            "tabMaquina" => Some(self.tabMaquina),
            _ => None,
        }
    }

    /// O TabItem que disparou o evento (`None` se o `sender` não é um TabItem deste form).
    pub fn sender_as_tab_item(&self, ui: &Ui, sender: ComponentId) -> Option<TabItem> {
        self.find_tab_item(ui.name(sender)?)
    }
}
