# Reflex — guia para agentes de IA

Aplicativo feito com o Aylis: a tela (forms, temas, imagens) é descrita em
JSON e o código dos eventos fica em Rust, na pasta `rust/`. Este arquivo foi criado
pelo "Novo projeto" da IDE do Aylis; pode editar à vontade (a IDE não o refaz).

## Onde está cada coisa

| Caminho | O que é |
|---|---|
| `Reflex.aylisproj` | O projeto: forms, tema, idioma, alvos e publicação |
| `forms/*.json` | Os forms (a tela), no formato do Aylis (`{"aylis": 1, "root": {...}}`) |
| `themes/`, `assets/`, `locales/` | O tema, as imagens, fontes e sons, e as traduções |
| `rust/generated/` | Gerado pela IDE a cada "Gerar código": **somente leitura** |
| `rust/` (o resto) | O código do app: **é seu** |

### No Reflex

O `rust/` é o `reflex.exe` inteiro, num crate só: a interface e o núcleo sem
tela, em módulos (`proto`, `auth`, `net`, `media`, `platform`, `host`,
`viewer`). O exe sai em `runtime/`, ao lado das DLLs; o Publicar gera
`dist/windows`. Texto em português do Brasil.

**Arquivos só pelo MCP do Aylis** (`edit_code`, `set_layout`, `save`...): criar e
editar, inclusive README, scripts e `.gitignore`. Mover e apagar, com `git mv`
e `git rm`. Arquivo novo pelo `edit_code`: o caminho é relativo a `rust/`
(`src/x.rs`).

| Caminho | O que é |
|---|---|
| `rust/src/modes.rs` | o `main`: interface, `--service`, `--agent`, `--user` e a CLI |
| `rust/src/sessao.rs` | uma conexão (viewer) sem tela; entrega quadros e cursor numa `Saida` |
| `rust/src/video/` | a janela de vídeo: geometria (pura), Win32 pelo `HWND`, gancho de teclado |
| `rust/src/maquina.rs`, `escolha.rs` | a aba "Esta máquina" e as listas de opções, sem tela |
| `forms/principal.json`, `forms/video.json` | a janela principal e a de vídeo |
| `rust/tests/` | os testes de integração (GPU, FFmpeg, rede: `#[ignore]`) |
| `runtime/` | o exe compilado e as DLLs (só o README vai pro git) |
| `scripts/` | baixar as DLLs e testar numa VM Hyper-V |

Regras de lá valem aqui: código limpo com testes, nada de lixo de build, o
que bloqueia (rede, pipe do serviço) fora da thread da tela. Os testes que
precisam de GPU, FFmpeg ou rede ficam `#[ignore]` (ver o README).

## O código em Rust

- Gerados (não edite): `rust/generated/` inteiro (as structs dos forms, o acesso tipado e o
  registro dos handlers). A IDE também acerta as features e os caminhos do `Cargo.toml` e põe
  os `mod` que faltam no `src/lib.rs`; o resto desses arquivos é seu.
- Seus: `rust/src/lib.rs` (o `app()`), `rust/src/main.rs`, `rust/src/forms/<form>.rs` (os
  eventos de cada form), os seus módulos e `rust/tests/`.
- Um handler novo entra no fim do `impl` do form:

  ```rust
  pub fn btnSalvar_click(&mut self, ui: &mut Ui, sender: ComponentId, args: &EventArgs) {
      let nome = self.edtNome.text(ui);
      self.lblResposta.set_text(ui, format!("Olá, {nome}!"));
  }
  ```

  Num evento-pergunta (`before_post`, `on_close_query`...) o handler devolve
  `Result<(), String>`: `Err(mensagem)` recusa (o `?` repassa).
- Outro form pelo tipo: `let form = FrmClientes::create(ui)?; ui.show(form)?;`.
- Mensagem: `message_dlg(ui, "Gravado.", DlgKind::Information, &[ModalResult::Ok], |_, _| {});`.
- Segundo plano: `ui.background(owner, move || trabalho(), move |ui, resultado| { ... })` roda o
  trabalho noutra thread e o fim na tela, só se o form `owner` ainda existir (no navegador, o
  trabalho roda na thread da tela). Com uma thread sua, volte à tela com
  `let poster = ui.poster();` e `poster.post(move |ui| ...)` (ou `post_to(owner, ...)`).
- Testes: `cargo test` na pasta `rust/` (`rust/tests/*.rs`): `let mut app = <crate>::app()?;`
  monta o app sem janela, e `app.ui.fire(id, "on_click", &EventArgs::None)` dispara um evento.

## Regras do código

- **Gerado × seu.** O que a IDE gera fica em `generated/` (e nos arquivos marcados como gerados
  acima) e é refeito a cada geração: não edite. O arquivo de cada form é seu; a IDE só acrescenta
  no fim dele os handlers novos que você ligar a um evento.
- **Acesso tipado.** Use os componentes pelo nome tipado gerado (exemplos acima), nunca
  procurando pelo nome em texto: renomear um componente vira erro de compilação, e não um erro
  em tempo de execução.
- **Nada de banco na tela.** Sem conexão, SQL ou driver no código da tela: o `ClientDataSet`
  guarda os dados e as alterações (o delta), e quem aplica no banco é o backend.
- **Mensagens ao usuário** pelo diálogo do Aylis (o `MessageDialog`/`message_dlg`), nunca a
  caixa de mensagem do sistema: ela sai igual em todas as plataformas e com o tema do app.
- **Eventos um por vez.** Os handlers rodam na thread da tela, um de cada vez. Trabalho demorado
  vai para outra thread, e o resultado volta à tela pelo `post`/`post_to` (veja acima); no
  navegador não há threads.
- **Erro num handler** recusa a ação (um evento-pergunta, como `before_post`, mostra a mensagem
  e não deixa gravar) e aparece na caixa de erro do Aylis: o app não cai.

## Forms e a IDE

Com o projeto aberto na IDE do Aylis, **não grave os arquivos do projeto direto** (forms, temas,
`.aylisproj`, código): a IDE guarda o que está aberto na memória e perderia a mudança ao salvar.
O `.mcp.json` desta pasta liga o agente à IDE (`aylis-tool mcp`): mude os forms com
`set_property`, `add_component`, `set_layout`/`patch_layout` (o `class_info` diz as propriedades e
os eventos de cada classe), o código com `read_code`/`edit_code`, e salve com `save`.

## Rodar e testar

- Pela IDE: F9 executa (no depurador, se houver ponto de parada), Ctrl+F9 compila. Pelo MCP:
  `build`, `run_without_debug`, `test`, `check` (a verificação de antes de compilar),
  `screenshot` (a foto de um form) e `app_command` (comanda o app em execução: clicar, digitar,
  ler a árvore).
- Pela linha de comando: `aylis-tool run Reflex.aylisproj`.
- Desempenho e uso: `profile` mede o app em execução (os handlers e os quadros mais lentos),
  `recording` resume a última gravação de uso e `coverage` diz que handlers nenhum teste ou uso
  executou.
