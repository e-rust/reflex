# Reflex — levantamento (controle remoto de tela em Rust)

Objetivo: um par **host** (máquina controlada, "servidor") e **viewer** (quem
controla, "cliente"), em Rust, reaproveitando o jeito de capturar e codificar
do NoOBS (`C:\Users\Eduar\OneDrive\Documentos\git\NoOBS\rust`).

> Terminologia: em acesso remoto "servidor" costuma ser a máquina **controlada**
> (ela serve a tela). Se a ideia for um servidor central que intermedeia
> conexões (como o RustDesk), isso é uma terceira peça: o **relay**. Ver as
> perguntas no fim.

---

## 1. O que o NoOBS já tem e serve aqui

O NoOBS hoje NÃO usa mais libobs: tem um motor próprio em `rust/src/capture/`
(ver `rust/docs/motor-proprio.md`). As peças que interessam:

| Peça do NoOBS | Arquivo | Serve pro Reflex? |
|---|---|---|
| Dispositivo D3D11 (multithread-protegido, `VIDEO_SUPPORT`) | `capture/gpu.rs` | Sim, igual |
| Captura WGC de monitor/janela, pool *free-threaded* consultado por `TryGetNextFrame` (sem callback) | `capture/video.rs` | Sim — é o coração do host |
| DXGI Desktop Duplication (reserva) | `capture/video.rs` | Sim, e passa a ser **importante** (dirty rects, cursor separado, tela do UAC — ver §4) |
| Compositor + conversor BGRA → NV12 BT.709 em shader | `capture/gpu.rs` | Sim; o viewer precisa do **inverso** (NV12 → BGRA) pra desenhar |
| Encoder libavcodec com quadro D3D11 sem sair da GPU (`h264_amf`, `hevc_nvenc`, `av1_amf`…) e fallback `libx264` por cópia | `capture/encode.rs`, `ffi/ffmpeg.rs` | Sim, com outra afinação (§2) |
| FFmpeg por `libloading` (DLLs em `exe\`) | `ffi/ffmpeg.rs` | Sim; acrescentar `avcodec` **decoder** + `d3d11va` |
| Detecção de encoders (DXGI + 1 quadro de teste) | `capture/caps.rs` | Sim |
| Relógio QPC | `capture/clock.rs` | Sim (timestamps pra medir latência) |
| Enumeração de monitores | `win_preview.rs` | Sim |
| Áudio WASAPI loopback + AAC | `capture/audio.rs` | Depois (Opus é melhor pra tempo real) |

Medições do NoOBS que já respondem perguntas: `h264_amf` 4K@30 em **0,7 ms/quadro**
na thread de vídeo (x264: 10 ms); WGC entrega mudança de tela em **< 30 ms**.

**O que muda de gravação para controle remoto:** gravação otimiza qualidade
por bit com grade de fps fixa; controle remoto otimiza **latência** e
**recuperação de perda**. Isso muda a afinação do encoder, a cadência e o
transporte — não a captura.

---

## 2. Codec

### 2.1 Escolha

| Opção | Latência | Texto nítido | Custo | Observação |
|---|---|---|---|---|
| **H.264 por hardware** (AMF/NVENC/QSV via libavcodec) | ~1–5 ms | médio (4:2:0) | quase zero de CPU | **Recomendado como padrão.** Decodifica em qualquer máquina |
| HEVC por hardware | ~1–5 ms | bom | idem | ~30% menos banda; decoder HW nem sempre existe no cliente |
| AV1 por hardware | ~2–6 ms | bom | idem | só GPUs novas (RX 7000+/RTX 40+/Arc) dos dois lados |
| H.264 4:4:4 (NVENC) | baixa | **ótimo** | — | AMF não faz 4:4:4; texto fica perfeito |
| `libx264` `zerolatency` | 5–15 ms em 1080p | médio | CPU alta | fallback universal (licença GPL!) |
| `openh264` (crate Rust, Cisco) | média | médio | CPU | sem FFmpeg; binário da Cisco cobre patente |
| Media Foundation (MFT H.264 do Windows) | baixa | médio | HW | sem FFmpeg, só `windows` crate; mais trabalhoso e varia por driver |
| Tiles + diff + zstd/lz4 (estilo VNC) ou JPEG por tile | baixa | **sem perda** (zstd) | CPU/banda alta em vídeo | ótimo pra MVP e pra telas paradas com texto |

**Recomendação:** H.264 por hardware via libavcodec (reaproveita `encode.rs`),
com `libx264` de reserva. Deixar o codec atrás de um trait (`VideoEncoder` /
`VideoDecoder`, igual o NoOBS faz) pra poder somar 4:4:4 e AV1 depois.

### 2.2 Afinação para tempo real (diferente do NoOBS)

- **Sem B-frames**, `max_b_frames=0`, sem *lookahead*.
- AMF: `usage=ultralowlatency`, `rc=cbr`; NVENC: `preset=p1`/`p2`,
  `tune=ull`, `zerolatency=1`, `delay=0`; x264: `tune=zerolatency`, `preset=ultrafast/superfast`.
- **CBR com VBV pequeno** (≈ 1 quadro) — evita rajada que entope a rede.
- **GOP infinito + IDR sob demanda** (cliente novo, perda de pacote) ou
  *intra-refresh* (espalha o I-frame em vários quadros, sem pico).
  ⚠️ O NoOBS anota que o AMF **não força keyframe** pelo libavcodec
  (`can_force_key: !amf`). Pra controle remoto isso é crítico: verificar a
  opção `forced_idr` na versão do FFmpeg em uso; se não existir, usar o
  `restart` do encoder (já existe em `encode.rs`) ou GOP curto (~2 s) no AMF.
- **Cadência variável**: só codificar quando a tela muda (WGC só entrega
  quadro novo quando muda; DXGI dá os *dirty rects*). Tela parada ≈ 0 kbps.
  Mandar um quadro de "refino" de qualidade alta ~200 ms depois que a tela
  para (texto fica nítido mesmo com bitrate baixo em movimento).
- **Bitrate adaptativo**: o viewer reporta perda/RTT; o host ajusta bitrate
  e, se preciso, resolução (reabrir encoder).
- **Cursor fora do vídeo**: `SetIsCursorCaptureEnabled(false)` e mandar
  forma (quando muda) + posição (sempre) num canal próprio; o viewer desenha
  localmente. Latência percebida do mouse cai muito. O DXGI já entrega a
  forma do ponteiro (`GetFramePointerShape`).

Banda típica H.264, desktop 1080p: 1–8 Mbps em uso, perto de 0 parado.

### 2.3 Decodificação (viewer)

- **libavcodec + `d3d11va`**: o quadro decodificado já é textura D3D11 NV12 →
  shader NV12→BGRA → *swapchain* (`DXGI_SWAP_EFFECT_FLIP_DISCARD`, *waitable
  object* pra 1 quadro de latência). Zero cópia. **Recomendado.**
- Software (`h264` do avcodec) → `sws_scale` pra BGRA → `StretchDIBits` (GDI):
  o "canvas simples". Bom pra MVP; em 4K pesa na CPU.

---

## 3. Rede

| Opção | Prós | Contras |
|---|---|---|
| **TCP + framing** (`tokio`, `tokio-rustls`) | simples, MVP rápido | perda = travada (*head-of-line*); ruim na internet |
| **QUIC** (`quinn` + `rustls`) | TLS embutido, vários *streams* sem *head-of-line*, **datagramas** não confiáveis pro vídeo, migração de conexão | um pouco mais de código |
| WebRTC (`webrtc-rs`) | NAT traversal (ICE/STUN/TURN), *jitter buffer*, controle de congestionamento prontos | pesado, menos controle |
| UDP próprio | controle total | reinventar congestionamento/criptografia |

**Recomendação:** começar com TCP na LAN (MVP), já com as mensagens
desenhadas pra migrar pra **QUIC** sem mudar o protocolo:

- *stream* confiável: controle, input, clipboard, cursor (forma);
- datagramas ou *stream* por quadro: vídeo (perdeu → pede IDR, não retransmite);
- serialização: `serde` + `postcard`/`bincode`.

**Internet/NAT:** máquinas atrás de NAT não aceitam conexão direta. Precisa
de um servidor público de **rendezvous** (troca endereços, tenta *hole
punching* UDP — QUIC ajuda) e **relay** quando o furo falha (é o
`hbbs`/`hbbr` do RustDesk). Só vale se o uso for fora da LAN.

**Segurança mínima:** TLS sempre; autenticação (senha/token de uso único +
ID da máquina); confirmação na tela do host (acesso assistido) ou senha fixa
(acesso não assistido); log de sessões.

---

## 4. Input e Windows

- **Injeção no host:** `SendInput` — mouse em coordenada absoluta com
  `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` (multi-monitor), teclado por
  **scancode** (`KEYEVENTF_SCANCODE`, independe do layout ABNT/US), roda,
  botões extras. `windows` crate (o NoOBS já tem `Win32_UI_Input_KeyboardAndMouse`).
  Crates prontas: `enigo` (injeção), `rdev` (escuta) — dá pra usar, mas o
  `SendInput` direto é pouca coisa.
- **Captura no viewer:** mensagens da janela + *hook* `WH_KEYBOARD_LL` quando
  a janela está em foco, pra mandar Win, Alt+Tab, Ctrl+Esc pro remoto.
- **Limites do Windows (importante decidir cedo):**
  - Processo comum **não** injeta input em janela elevada (UIPI) nem vê a
    **área de trabalho segura** (prompt do UAC, Ctrl+Alt+Del, tela de login).
    WGC não captura essa tela.
  - Pra acesso não assistido completo é preciso um **serviço do Windows**
    (SYSTEM) que lança o host na sessão do usuário (`WTSQueryUserToken` /
    `CreateProcessAsUser`), alterna com `OpenInputDesktop`/`SetThreadDesktop`
    e usa **DXGI Duplication** quando a área de trabalho é a Winlogon.
    Ctrl+Alt+Del remoto = `SendSAS` (exige serviço).
  - O NoOBS já tem `elevate.rs` e `Win32_System_RemoteDesktop` — ponto de partida.
- Extras: clipboard (texto, depois arquivos), troca de monitor, bloqueio de
  input local, áudio (WASAPI loopback do NoOBS + Opus).

### 4.1 Tela bloqueada / login (requisito confirmado)

O host precisa funcionar com o Windows bloqueado e na tela de login (inclusive
logo depois do boot, sem ninguém logado). É possível — é como RustDesk,
AnyDesk e TeamViewer fazem — com **duas peças**:

```
Sessão 0 (sem tela)                       Sessão do console (1, 2…)
┌──────────────────────────┐   pipe       ┌──────────────────────────────┐
│ reflex-service (SYSTEM)  │◄────────────►│ reflex-agent (SYSTEM, token  │
│ - sobe no boot           │  nomeado     │   do serviço com SessionId   │
│ - rede, autenticação     │              │   trocado)                   │
│ - vigia troca de sessão  │              │ - captura DXGI / WGC         │
│ - (re)lança o agente     │              │ - encode, cursor             │
└──────────────────────────┘              │ - SendInput, clipboard       │
                                          └──────────────────────────────┘
```

- **Serviço** (crate `windows-service`), conta LocalSystem, início automático.
  A sessão 0 não tem tela nem placa de vídeo útil: o serviço **não captura**.
  Ele cuida da rede e da autenticação (assim a máquina atende antes do login)
  e lança o agente.
- **Lançar o agente na sessão certa:** `WTSGetActiveConsoleSessionId` →
  `DuplicateTokenEx` do token do próprio serviço → `SetTokenInformation(TokenSessionId)`
  → `CreateProcessAsUserW` com `lpDesktop = "winsta0\\winlogon"` (ou `default`).
  Com token SYSTEM o agente pode entrar em **qualquer** área de trabalho da
  sessão, inclusive a Winlogon (login, bloqueio, UAC).
- **Trocar de área de trabalho:** a cada quadro/evento o agente confere
  `OpenInputDesktop` + `GetUserObjectInformationW(UOI_NAME)`; se mudou
  (`Default` ↔ `Winlogon` ↔ `Screen-saver`), faz `SetThreadDesktop` na
  thread de captura e na de input e recria a captura.
- **Captura na tela de login: DXGI Desktop Duplication**, não WGC (o WGC não
  enxerga a área de trabalho segura). O DXGI devolve `DXGI_ERROR_ACCESS_LOST`
  na troca de área de trabalho → recriar o duplicador. Por isso o DXGI, que
  era "reserva" no NoOBS, vira o **caminho principal** do host; o WGC fica
  opcional (captura de janela).
- **Input:** `SendInput` vindo de um processo SYSTEM na área de trabalho
  Winlogon funciona (digitar senha/PIN). Também resolve o UIPI (janelas
  elevadas).
- **Ctrl+Alt+Del:** `SendSAS(FALSE)` (sas.dll), chamado pelo serviço; exige
  a política `SoftwareSASGeneration` = 1 ou 3
  (`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System`), que o
  instalador grava.
- **Troca de sessão:** o serviço recebe `SERVICE_CONTROL_SESSIONCHANGE`
  (logon, logoff, lock, unlock, troca rápida de usuário, console ↔ RDP) e
  mata/relança o agente na nova sessão de console.
- **Encode por hardware** funciona no agente SYSTEM dentro da sessão do
  console (AMF/NVENC/QSV); só não funcionaria na sessão 0.

Cuidados:
- Um serviço SYSTEM escutando na rede é alvo valioso: autenticação forte,
  TLS obrigatório, sem comando além de vídeo/input/clipboard, binários
  assinados, pipe nomeado com ACL só pra SYSTEM.
- Instalação exige admin (instalador NSIS como o do NoOBS: `sc create`,
  política do SAS, regra de firewall).
- Notebooks híbridos (Intel + NVIDIA/AMD): o DXGI precisa do adaptador que
  está ligado ao monitor (enumerar saídas por adaptador).
- Modo "sem serviço" (portátil, acesso assistido) continua possível: o
  mesmo agente rodando como usuário comum, sem tela de login/UAC.

### 4.2 Servidores e máquinas sem monitor (requisito confirmado)

Sem monitor ligado, a placa pode não ter **nenhuma saída ativa**: o DXGI não
tem o que duplicar e o WGC não tem monitor pra capturar. Os cenários:

| Máquina | O que acontece sem monitor | Solução |
|---|---|---|
| PC/servidor com GPU (AMD/NVIDIA/Intel) | sem saída ativa → nada pra capturar | **monitor virtual (IddCx)** apontando pra essa GPU como *render adapter* → encode HW continua |
| Servidor sem GPU (vídeo onboard ASPEED/Matrox do BMC) | às vezes o driver mantém um modo padrão (1024x768), às vezes não; **testar** | monitor virtual com render em software (WARP) → **encode por CPU** |
| VM (Hyper-V, VMware, Proxmox/KVM) | **Hyper-V: o monitor só existe com a janela do console (VMConnect) aberta** — fechou, o `HyperVMonitor` some (PnP `presente=False`), o Windows para de desenhar e o DXGI só entrega preto (medido numa VM Windows 11). VMware/KVM: testar | **monitor virtual**; encode por CPU (sem GPU passada pra VM) |
| Notebook com a tampa fechada | o painel desliga | monitor virtual |
| Alternativa de hardware | — | *dummy plug* HDMI/DP (~R$30): a placa acha que tem monitor. Bom pra testar, ruim pra distribuir |

**Implementado (v1):** o *Virtual Display Driver* (VirtualDrivers, MIT,
assinado pela SignPath) vai embutido no `reflex.exe` (`third_party/vdd`) e o
`--install` instala sem perguntar nada (o assinante entra em "Editores
Confiáveis" só durante a instalação). O serviço liga o monitor virtual quando
não há monitor de verdade e desliga quando volta um (2 voltas de 2 s de
confirmação). Medido na VM Hyper-V: console fechado → 1920×1080 a ~25 quadros/s;
abrir/fechar o console troca de monitor em 2–4 s sem derrubar a conexão.

**Monitor virtual = driver Indirect Display (IddCx).** Driver em modo
usuário (UMDF) que cria um monitor que não existe; o Windows desenha nele
como num monitor de verdade e o **DXGI captura normalmente** — o resto do
host não muda. Bônus: a resolução pode ser a que o viewer pedir.
- Referências abertas: `IddSampleDriver` (Microsoft, C++), *Virtual Display
  Driver* (MttVDD, fork mantido do sample), `virtual-display-rs` (IddCx em
  **Rust**). O Parsec VDD e o Sunshine usam a mesma técnica.
- Caminho mais simples: o driver só **cria o monitor**; a captura segue pelo
  DXGI do agente. (Dá pra o próprio driver entregar os quadros, como o
  Parsec faz, mas é mais complexo — fica pra depois, se precisar.)
- O agente/serviço liga o monitor virtual quando não acha saída ativa (ou
  sempre, por configuração) e desliga quando há monitor físico.
- ⚠️ **Assinatura:** pra instalar fora de modo de teste, o driver precisa de
  certificado EV + assinatura de atestado da Microsoft (Partner Center).
  Isso tem custo e prazo — planejar cedo. Em desenvolvimento: `bcdedit /set testsigning on`.
- ⚠️ Verificar suporte do IddCx nas versões de **Windows Server** alvo
  (2019/2022/2025) — os projetos acima relatam uso em Server, mas confirmar.

**Encode por CPU vira obrigatório** (servidor sem GPU, VMs):
- `openh264` (BSD, binário da Cisco cobre a patente) ou `libx264`
  `zerolatency` (GPL). 1080p30 de desktop cabe folgado numa CPU de servidor,
  ainda mais com cadência variável (tela parada não codifica).
- Media Foundation tem encoder H.264 em software, mas no **Windows Server**
  exige o recurso *Media Foundation* instalado (`Server-Media-Foundation`);
  não dá pra contar com ele.
- A detecção de encoders do NoOBS (`caps.rs`) escolhe sozinha HW ou CPU.

**Sessões RDP em servidor:** é comum o admin entrar por RDP. Quando ele
desconecta, a sessão sai do console e o agente perde a tela. O serviço deve
seguir a **sessão de console** (já previsto em §4.1) e, opcionalmente,
devolver a sessão ao console ao desconectar (`WTSConnectSession`/`tscon`).
Windows Server Core não tem área de trabalho completa — fora do escopo.

---

## 5. Interface

**Rust não tem VCL nativa.** O que existe:

| Opção | Estilo | Designer visual? | Situação | Para o Reflex |
|---|---|---|---|---|
| **Win32 puro** (`windows` crate) | controles nativos, `CreateWindowEx`, `WM_PAINT` | não (ou `.rc`) | o NoOBS já usa | **janela do viewer** (swapchain D3D11 nela) |
| **winsafe** | Win32 nativo com API segura; diálogos de **arquivo `.rc`** | **sim** — desenha o diálogo num editor de recursos (ex.: ResEdit/Visual Studio) | mantido | **formulários** (conectar, senha, config): o mais perto de "desenhar o form" |
| native-windows-gui (NWG) | controles nativos, parecido com VCL | não | **pouco mantido** | evitar em projeto novo |
| fltk-rs | FLTK (não nativo, leve, exe pequeno) | **sim** — FLUID + `fl2rust` | mantido | alternativa cross-platform com designer |
| egui / eframe | *immediate mode*, desenha tudo | não | muito ativo | ótimo pra painel/overlay por cima do vídeo e telas de debug |
| Slint | declarativo (`.slint`) | *live preview* no VS Code | ativo; licença GPL/royalty-free/comercial | bonito, mas mais uma linguagem |
| iced | estilo Elm | não | ativo | sem vantagem aqui |
| WebView2 (como o NoOBS) | HTML/CSS | — | vocês já dominam | bom pros menus; **não** pro vídeo (latência e cópias) |
| rust-vcl (liblcl/Lazarus) | LCL do Lazarus via DLL | Lazarus | **experimental/abandonado** | curiosidade, não usar |

### 5.1 Decisão: winsafe (controles nativos) + D3D11 na área de vídeo

Sem navegador nem WebView2. Tudo nativo do Windows:

- **`winsafe`** para janelas, formulários e controles: é Win32 de verdade
  (Button, Edit, ComboBox, **ListView**, TreeView, Tab, ProgressBar,
  StatusBar, menus, ícone de bandeja), com API segura em Rust. Aparência do
  Windows, exe pequeno, nenhum runtime. Formulários podem ser **desenhados
  visualmente** como diálogo em arquivo `.rc` (editor de recursos do Visual
  Studio ou ResEdit) e carregados pelo winsafe — o mais perto da experiência
  VCL/Delphi.
- **Área de vídeo:** um controle filho (janela comum dentro do formulário)
  onde o crate `windows` cria a **swapchain D3D11** e desenha o quadro
  decodificado (shader NV12→BGRA). O winsafe entrega o `HWND`; a ponte pro
  `windows` é só converter o ponteiro. É o "canvas", só que na GPU.
- **Por que não os outros:** egui/iced/Slint desenham os próprios controles
  (não parecem Windows, e o painel de arquivos fica pior que um ListView
  nativo); NWG está pouco mantido; Win32 cru com o `windows` crate funciona,
  mas é bem mais verboso que o winsafe pra formulários.
- **MVP:** antes da swapchain, o mesmo controle de vídeo pode desenhar por
  `StretchDIBits` no `WM_PAINT` (decode por software). Trocar depois não
  mexe no resto da interface.

### 5.1.1 E o WinUI 3? — **descartado: winsafe + retoques do Windows 11**

**Decidido:** winsafe com os retoques abaixo, no app único (§5.3). Mais nativo e leve, Rust puro, sem runtime extra (bom pra servidor).
O WinUI 3 fica fora; a tabela abaixo fica como registro do porquê.

Retoques (tudo via `windows`/winsafe, sem dependência nova):
- **Manifesto:** Common Controls v6 (visual atual dos controles), DPI
  *per-monitor v2*, `supportedOS` do Windows 10/11.
- **Moldura Windows 11** (`DwmSetWindowAttribute`): fundo **Mica**
  (`DWMWA_SYSTEMBACKDROP_TYPE`, Win11 22H2+), barra de título escura
  (`DWMWA_USE_IMMERSIVE_DARK_MODE`) seguindo o tema do sistema, cantos
  arredondados (`DWMWA_WINDOW_CORNER_PREFERENCE`). No Windows 10/Server sem
  suporte, as chamadas falham e fica a moldura padrão.
- **Fonte e ícones:** Segoe UI Variable (Segoe UI de reserva) e ícones da
  fonte **Segoe Fluent Icons** (MDL2 no Win10) na barra de conexão e nas
  toolbars — os mesmos glifos do WinUI.
- **Modo escuro nos controles:** parcial — `SetWindowTheme(hwnd,
  "DarkMode_Explorer")` em ListView/TreeView/barras de rolagem e cores
  próprias nos campos (`WM_CTLCOLOR*`). A Microsoft não documenta modo
  escuro completo pra Win32; começar no tema claro e tratar o escuro depois.
- **Barra de conexão da sessão:** desenhada por nós (GDI/Direct2D) por cima
  do vídeo, no estilo Fluent — é a parte mais visível e não depende dos
  controles clássicos.

<details><summary>Alternativas avaliadas</summary>

Dá, mas não é direto. O WinUI 3 vem do **Windows App SDK**, não do Windows
SDK: o crate `windows` não traz `Microsoft.UI.Xaml`, e a Microsoft arquivou
o projeto experimental que fazia isso em Rust (`windows-app-rs`).

| Caminho | Visual | Custo / risco |
|---|---|---|
| **WinUI 3 direto do Rust** (bindings gerados com `windows-bindgen` a partir dos `.winmd` do Windows App SDK) | WinUI 3 real | sem compilador XAML pra Rust: telas montadas em código ou `XamlReader.Load` em tempo de execução; implementar à mão `Application`/`IXamlMetadataProvider`; inicializar o runtime (`MddBootstrapInitialize`); quase ninguém faz → **alto risco** |
| **Interface em C# WinUI 3 + núcleo Rust em DLL** (C ABI; `csbindgen`/`uniffi`) | WinUI 3 real, XAML, *hot reload*, Visual Studio | duas linguagens; runtime .NET + Windows App SDK (dá pra empacotar junto, exe/instalador maior). Vídeo: `SwapChainPanel` recebe a swapchain D3D11 criada pelo Rust (`CreateSwapChainForComposition` + `ISwapChainPanelNative`) — caminho suportado |
| **Slint com estilo `fluent`** (Rust puro) | **imita** o WinUI 3 (é o estilo padrão no Windows) | não é nativo; licença GPL / *royalty-free* com atribuição / comercial; vídeo D3D11 exige janela filha por cima ou cópia pra CPU |
| **winsafe + retoques do Windows 11** (Mica, modo escuro, cantos arredondados via `DwmSetWindowAttribute`) | Win32 nativo com cara de Windows 11 na moldura; controles no estilo clássico do Windows | o mais barato e leve; não é WinUI |

Ponto a favor de manter algo leve **no host**: o `reflex-config` roda em
servidores, onde instalar o runtime do Windows App SDK é mais um requisito
(verificar suporte nas versões de Windows Server alvo). Dá pra misturar:
**viewer em WinUI 3** (C#) e **config do host em winsafe**.

</details>

### 5.2 Telas

Um app só, `reflex.exe` (§5.3), com duas abas na janela principal:
**Conectar** (controlar outra máquina) e **Esta máquina** (permitir ser
controlada).

**Aba Conectar:**

| Tela | Conteúdo |
|---|---|
| Principal | campo **IP:porta** + Conectar; ListView dos hosts salvos (apelido, endereço, último acesso, codec); Novo / Editar / Excluir |
| Confirmar host | 1ª conexão: impressão digital do certificado (TOFU) pra aceitar; alerta se mudou |
| Minha identidade | o **ID de acesso** deste viewer (impressão digital da chave, §11.1) pra copiar e mandar a quem vai cadastrar |
| Código de acesso | quando não há cadastro: campo pro código/link de acesso único (§11.4) |
| **Sessão** | área de vídeo D3D11 ocupando tudo; **barra de conexão** no topo (estilo RDP, recolhe sozinha, pode fixar): monitor, codec, qualidade/fps, Ctrl+Alt+Del, clipboard liga/desliga, arquivos, tela cheia, desconectar; **Gerenciar máquina** (só master: abre a aba Esta máquina do host remoto pela API, §11.5); barra de status opcional com latência, fps, kbps, codec, HW/SW |
| Arquivos | duas ListViews (local × remoto) com caminho e toolbar; fila de transferências com ProgressBar e cancelar |
| Opções do host | apelido, endereço, codec preferido, clipboard (2 sentidos/1/desligado), escala (ajustar/1:1), transporte (auto/QUIC/TCP) |

**Aba Esta máquina** (fala com o serviço por pipe; ver para alterar = admin, §5.3):

| Tela | Conteúdo |
|---|---|
| Geral | **Permitir acesso a esta máquina** (liga/desliga = instala/inicia ou para o serviço); estado do serviço, IP(s) e porta pra informar no outro lado, impressão digital pra conferir |
| **Compartilhar acesso** | chave liga/desliga (§11.4): código atual + link pra copiar, nível dos convidados, contagem até o próximo código (troca a cada 10 min e a cada uso), Desconectar convidados |
| **Usuários** | ListView (nome, ID de acesso, nível, pede confirmação?, último acesso); Adicionar / Editar / Remover; **master** em destaque e sem botão de remover (§11.3) |
| Segurança | permitir clipboard / arquivos / só leitura, pasta raiz dos arquivos |
| Vídeo | codecs permitidos, monitor virtual (auto/sempre/nunca), limite de fps/bitrate |
| Sessões | log de conexões (quem, quando, de qual IP) |

**Na máquina controlada durante a sessão:** ícone na bandeja + aviso
"sendo acessado de 192.168.x.x" (o `reflex.exe --user` mostra na sessão do
usuário), com opção de desconectar. Configurável, mas ligado por padrão.

### 5.3 Um executável só

Viewer, configuração e as peças do host são **o mesmo `reflex.exe`**; o modo
vem da linha de comando (como AnyDesk e RustDesk):

| Comando | Quem roda | Papel |
|---|---|---|
| `reflex.exe` | o usuário, pelo atalho | janela principal: abas Conectar e Esta máquina |
| `reflex.exe --service` | o Windows (serviço, SYSTEM, sessão 0) | rede, autenticação, lança os outros dois (§4.1) |
| `reflex.exe --agent` | o serviço (SYSTEM, sessão do console) | captura, encode, input, tela de login |
| `reflex.exe --user` | o serviço (token do usuário logado) | clipboard, arquivos, ícone/aviso de sessão (§10.8.2) |

- **Um arquivo, uma versão:** não existe viewer de uma versão falando com
  host de outra na mesma máquina; atualizar = trocar um exe.
- **Ser controlada é opcional:** quem só controla nunca liga o serviço. O
  botão "Permitir acesso a esta máquina" instala/inicia o serviço (o mesmo
  exe com `--service`) e pede elevação (UAC) só nessa hora.
- **Configurar sem rodar tudo como admin:** a aba Esta máquina lê o estado
  pelo pipe do serviço; o serviço só **aceita alterações** de quem é
  administrador (confere o token do cliente do pipe:
  `ImpersonateNamedPipeClient` + `CheckTokenMembership`). Botões de
  alteração com o escudo do UAC; sem admin, a aba é só leitura.
- **Custo:** o exe carrega o código das duas pontas (alguns MB a mais); as
  DLLs do FFmpeg já são compartilhadas de qualquer jeito.
- **Instalador:** um só (NSIS), com a opção "permitir acesso a esta máquina"
  desmarcável na instalação.

---

## 6. Projetos de referência

| Projeto | Linguagem / licença | O que olhar |
|---|---|---|
| **RustDesk** | Rust, **AGPL-3.0** | arquitetura completa: captura (`scrap`), codecs (vpx/aom/hwcodec), input, serviço, `hbbs`/`hbbr` (rendezvous/relay). **Não copiar código** se o Reflex não for AGPL |
| Sunshine + Moonlight | C++, GPL-3.0 | afinação de NVENC/AMF/QSV pra latência mínima, controle de perda, FEC |
| Apache Guacamole / noVNC | Java/JS, Apache-2 | protocolo de tiles, RDP/VNC |
| Crates: `windows-capture`, `xcap`, `scrap` | Rust | captura (o NoOBS já tem algo melhor) |
| Crates: `quinn`, `tokio`, `rustls`, `postcard`, `enigo`, `openh264`, `opus` | Rust | rede, input, codecs alternativos |

⚠️ **Licença do FFmpeg:** se as DLLs do NoOBS forem build GPL (com `libx264`),
o Reflex distribuído com elas fica GPL. Pra uso comercial/fechado: build
**LGPL** (sem x264) e só encoders de hardware, ou `openh264`.

---

## 6.1 Alternativas ao modelo "NoOBS + protocolo próprio"

O modelo do NoOBS (captura D3D11 → encoder HW pelo libavcodec) é bom para a
**captura**. Para o resto, há caminhos que podem ser melhores:

| Alternativa | O que ganha | O que custa | Veredito |
|---|---|---|---|
| **WebRTC no host** (`str0m`, Rust, sans-IO; ou `webrtc-rs`) com **viewer no navegador** | viewer sem instalar nada (Chrome/Edge decodificam H.264 por HW); NAT traversal (ICE/STUN/TURN), controle de congestionamento e jitter buffer prontos; input pelo DataChannel | sinalização (um servidor web pequeno), menos controle fino da latência, teclas como Win/Alt+Tab o navegador não entrega | **Forte candidato se for uso pela internet** |
| **Host falando RDP** (IronRDP — Rust, Devolutions, tem lado servidor; verificar maturidade) | qualquer cliente RDP conecta (mstsc, apps de celular, FreeRDP); codecs maduros do protocolo (RemoteFX, **AVC444**) | protocolo grande e complexo; o lado servidor do IronRDP é bem menos usado que o cliente | interessante, arriscado; avaliar depois do MVP |
| **AVC444** (truque do RDP: dois streams H.264 4:2:0 recompõem 4:4:4) | texto nítido usando encoder HW 4:2:0 comum (AMF incluído) | dois encodes por quadro + shader de recomposição no viewer | **adotar como melhoria** de qualidade, com ou sem RDP |
| **SDKs dos fabricantes** (AMF, NVENC, oneVPL) direto, sem FFmpeg | controle total: IDR/intra-refresh sob demanda (resolve o risco do AMF), latência mínima, sem DLLs do FFmpeg nem dúvida de GPL | três integrações em vez de uma | **plano B do encoder**: começar com libavcodec; trocar o backend se o IDR/latência não fecharem |
| **Media Foundation** (MFT H.264 HW do Windows) | sem FFmpeg, sem licença, encoder e decoder do sistema | API verbosa, comportamento varia por driver | reserva leve, junto com `openh264` |
| **Monitor virtual** (driver IddCx, como o Parsec VDD / usado pelo Sunshine) | host **sem monitor** (servidor, mini PC, tampa fechada) continua capturável; resolução escolhida pelo viewer | driver assinado pra instalar | **adotado** — requisito de servidores sem monitor (§4.2) |
| **Windows Desktop Sharing API** (`rdpencom`, a base da Assistência Remota) | compartilhamento pronto em COM, quase sem código | não mostra a tela de login/UAC, codecs antigos | **descartado** pelo requisito da §4.1 |
| **RustDesk self-hosted** (só usar, sem construir) | tudo pronto hoje, inclusive serviço/login e relay | AGPL, pouco controle, não é produto seu | vale como **comparação de referência**; se o objetivo for só "ter a ferramenta", ele já resolve |

**Proposta revista:**
- captura do NoOBS, com **DXGI** como principal (§4.1) e monitor virtual se preciso;
- encoder pelo libavcodec agora, atrás de trait, com SDK direto como plano B;
- **protocolo:** LAN e viewer nativo → protocolo próprio sobre QUIC (§3);
  internet e/ou "abrir no navegador" → **WebRTC** (`str0m`) com viewer web,
  e um viewer nativo opcional depois (pra capturar Win/Alt+Tab). É a decisão
  que mais depende da pergunta 1 da §9;
- AVC444 como melhoria de texto depois do MVP.

---

## 7. Arquitetura proposta

```
reflex/                      (workspace Cargo)
  crates/
    proto/      mensagens (serde): Hello, Auth, VideoPacket, CursorShape,
                CursorPos, Input{Mouse,Key,Wheel}, Clipboard, RequestIdr, Stats
    capture/    porte de NoOBS capture/{gpu,video,clock,caps}: WGC + DXGI,
                D3D11, BGRA→NV12, dirty rects, cursor separado
    codec/      traits VideoEncoder/VideoDecoder; impl libavcodec (HW + x264),
                depois openh264/MF; NV12→BGRA pro viewer
    net/        transporte atrás de trait: TCP (MVP) → QUIC (quinn)
    agent/      lib: modo --agent, sessão do console (SYSTEM): captura →
                encode; input → SendInput; troca Default ↔ Winlogon
    service/    lib: modo --service (sessão 0): rede + auth, lança --agent e
                --user por sessão, SendSAS, pipe de config (§4.1, §5.3)
    user/       lib: modo --user, como o usuário logado: clipboard, arquivos,
                aviso de sessão (§10.8.2)
    clipfiles/  IDataObject/IStream de arquivos virtuais (viewer e user)
    viewer/     lib: conecta → recebe → decode → swapchain; captura input
    ui/         lib: janela principal winsafe (abas Conectar / Esta máquina)
  app/          bin único reflex.exe: escolhe o modo pela linha de comando (§5.3)
  driver/       monitor virtual IddCx (fork do MttVDD/IddSampleDriver ou
                virtual-display-rs), instalado junto com o serviço (§4.2)
    relay/      (opcional) rendezvous + relay p/ internet
```

Threads no host (mesma filosofia do NoOBS): vídeo (poll WGC → NV12 → encode),
rede (envio/recebimento), input (injeção). No viewer: rede, decode+present,
UI/input.

### Orçamento de latência (LAN, alvo < 50 ms "do vidro ao vidro")

captura WGC (até 1 quadro, ~16 ms a 60 Hz) + NV12/encode HW (~1–5 ms) +
rede LAN (~1 ms) + decode HW (~2–5 ms) + apresentação (1 vsync, ~16 ms).

---

## 8. Fases sugeridas

1. **Loopback num processo só:** captura **DXGI** → encode → decode → janela
   Win32. Medir latência (timestamp QPC no pacote) e CPU/GPU. Valida
   reaproveitamento do NoOBS e a afinação de baixa latência (incluindo o
   problema do IDR no AMF).
1b. **Prova de conceito da tela de login** (cedo, porque define a
   arquitetura): serviço mínimo que lança o agente na sessão do console, o
   agente grava alguns quadros da área de trabalho Winlogon (bloquear com
   Win+L) e injeta uma tecla.
1c. **Prova de conceito sem monitor**: instalar um IddCx pronto (MttVDD) em
   modo de teste, desligar o monitor físico e capturar pelo DXGI; repetir
   numa VM e, se houver, num servidor sem GPU (encode por CPU).
2. **Host/viewer por QUIC na LAN** (direto, sem passar por TCP — o `quinn`
   não é muito mais difícil), só vídeo, IDR sob demanda, contrapressão (§10.4).
3. **Input** (mouse absoluto, teclado por scancode) + cursor separado.
4. **Acesso** (§11): identidade do viewer + TLS mútuo, usuários e níveis,
   master, convite de acesso único (SPAKE2), API de administração remota;
   transporte TCP de reserva (§10.5), escolha de codec (§10.6), bitrate
   adaptativo, estatísticas na tela.
5. Multi-monitor, clipboard (§10.7) e **copiar/colar arquivos pelo
   clipboard** (§10.8 — obrigatório na 1ª versão), com o `reflex.exe --user`.
6. Serviço completo: troca de sessão, UAC, Ctrl+Alt+Del (`SendSAS`),
   instalador com política e firewall. **Fim da 1ª versão.**
7. (depois) arrastar e soltar, painel de arquivos, áudio (Opus), quadro de
   refino, AVC444.
8. (se for internet) rendezvous + relay.

---

## 9. Decisões em aberto (preciso de você)

1. ~~Onde roda~~ **Decidido:** só LAN por enquanto → protocolo próprio
   sobre QUIC + viewer nativo (§10). WebRTC/relay ficam pra quando houver
   internet.
2. ~~Acesso assistido ou não assistido?~~ **Decidido:** precisa funcionar
   bloqueado/na tela de login → serviço + agente (§4.1).
3. ~~"Servidor"~~ **Decidido:** na LAN não há servidor central; "servidor" =
   máquina controlada (serviço + agente).
4. **Licença/distribuição:** uso pessoal/interno, open source, ou comercial?
   (define FFmpeg GPL vs LGPL e se dá pra se inspirar em código do RustDesk)
5. ~~GPUs alvo~~ **Decidido em parte:** servidores e máquinas sem monitor
   entram → monitor virtual + encode por CPU obrigatórios (§4.2). Em aberto:
   quais versões de Windows Server e se há orçamento pra assinar o driver
   (certificado EV).
6. **Só Windows** nos dois lados, ou viewer em outros sistemas no futuro?
   (se sim, pesa a favor de egui/fltk/Slint no viewer em vez de Win32)
7. **Reaproveitar código do NoOBS** copiando os módulos, ou extrair um crate
   comum (`capture`) que os dois projetos usam?

---

## 10. Protocolo (LAN, QUIC) — primeira versão

Crates: `quinn` (QUIC), `rustls` (TLS 1.3), `tokio`, `serde` + `postcard`
(mensagens), `rcgen` (certificado).

### 10.1 Conexão

- **UDP 47800** (configurável); o instalador abre a regra no firewall.
- ALPN `reflex/1`. A versão do protocolo vai também no `Hello`.
- **Certificado:** o serviço gera um par autoassinado na instalação
  (`rcgen`), guardado com ACL só pra SYSTEM. Na LAN não há CA.
- **Confiança:** *TOFU* (como o SSH) — na 1ª conexão o viewer mostra a
  impressão digital (SHA-256 do certificado, em grupos curtos) e guarda;
  depois, mudou = alerta e recusa.
- **Autenticação:** ver §11 — usuário cadastrado entra pela **chave do
  viewer** (TLS mútuo, sem senha); convite de acesso único entra pelo
  **código** (SPAKE2 amarrado ao canal TLS pelo *exporter* do rustls: o
  código nunca trafega e um MITM não consegue se passar pelo host).
- **Sem descoberta/anúncio:** o host não se anuncia na rede. O viewer
  conecta só informando **IP (ou nome DNS) + porta**; ele guarda a lista de
  hosts já usados (endereço, apelido, impressão digital).

### 10.2 Canais (streams QUIC)

| Canal | Tipo | Direção | Conteúdo |
|---|---|---|---|
| **controle** | bidirecional, confiável | ↔ | `Hello`, `Auth`, `SessionConfig`/`ConfigApplied`, `MonitorList`, `RequestKeyframe`, `Stats`, `Ping`/`Pong`, `ClipboardOffer`, `Bye` |
| **clipboard (dados)** | um bidirecional **por pedido** | ↔ | `ClipboardRequest` → `ClipboardData` (§10.7) |
| **arquivos** | um bidirecional **por operação**, prioridade baixa | ↔ | listar pasta, enviar, baixar (§10.8) |
| **vídeo** | unidirecional, confiável | host → viewer | `VideoFrame` (MVP: um stream só; ver §10.4) |
| **input** | unidirecional, confiável e ordenado | viewer → host | `Mouse`, `Wheel`, `Key`, `ReleaseAll` (perder um *key up* = tecla presa) |
| **cursor** | unidirecional (forma) + **datagrama** (posição) | host → viewer | `CursorShape` (raro, confiável); `CursorPos` em datagrama: só vale o mais novo |

Separar em streams evita que um quadro grande de vídeo atrase input ou
controle (não há *head-of-line* entre streams no QUIC).

### 10.3 Mensagens

Enquadramento: `u32` (tamanho, little-endian) + mensagem `postcard`. Enums
com variante `Unknown` pra versões futuras não derrubarem a conexão.

```rust
// controle
Hello { proto: u16, app_version: String, caps: Caps }      // ambos
Caps { encoders: Vec<CodecCap>, decoders: Vec<CodecCap>, max_w: u32, max_h: u32 }
CodecCap { codec: Codec /*H264|Hevc|Av1*/, hw: bool, max_w: u32, max_h: u32 }
// autenticação (§11): a chave já foi provada no TLS; convite usa SPAKE2
AuthInvite { spake2_msg: Vec<u8> } ↔ AuthInviteReply { spake2_msg, confirm }
AuthResult { ok: bool, session_id: u64, level: Level, perms: Perms }
SessionConfig { monitor: u32, codec: CodecChoice /*Auto|H264|Hevc|Av1*/, max_fps: u8,
                bitrate_kbps: u32, max_w: u32, max_h: u32 }
MonitorList(Vec<Monitor { id, name, x, y, w, h, primary, virtual_: bool }>)
RequestKeyframe { reason: KeyReason }        // entrada, perda, decoder perdido
Stats { rtt_us, decode_us, present_us, frames_dropped, kbps }   // viewer → host
Ping { t_ns } / Pong { t_ns, host_t_ns }     // RTT + offset de relógio

// vídeo
VideoFrame { seq: u64, capture_ns: u64, flags: u8 /*key, size_changed*/,
             w: u16, h: u16, codec: Codec, data: Vec<u8> }

// input (coordenadas normalizadas 0..65535 no monitor escolhido)
Mouse { x: u16, y: u16, buttons: u8 }        // estado completo, não delta
Wheel { dx: i16, dy: i16 }
Key { scancode: u16, extended: bool, down: bool }
ReleaseAll                                     // viewer perdeu o foco

// cursor
CursorShape { id: u32, w, h, hot_x, hot_y, kind /*cor/mono/xor*/, pixels }
CursorPos { x: u16, y: u16, visible: bool, shape_id: u32 }   // datagrama
```

- Input como **estado completo** (posição absoluta + botões) e não delta: se
  algo se perder ou reordenar, o próximo evento corrige.
- `capture_ns` + offset do `Ping/Pong` = latência de vidro a vidro exibida
  nas estatísticas do viewer.
- Sequência ao conectar: `Hello` ↔ `Hello` → `Auth` → `AuthResult` →
  `MonitorList` → `SessionConfig` → host abre os streams de vídeo e cursor e
  manda um keyframe.

### 10.4 Latência: nunca enfileirar quadro

A regra que mais pesa na latência: **o host não acumula quadros**.

- Antes de codificar, o host olha quanto ainda falta enviar no stream de
  vídeo. Se passou de ~1 quadro, **pula a captura deste tique** (o
  `quinn` com controle de congestionamento BBR mede a banda). Com cadência
  variável, o próximo quadro já leva a tela atual.
- Pular quadro *antes* do encoder não quebra a cadeia de referência do
  H.264; descartar *depois* quebraria (exigiria keyframe).
- Na LAN, perda de pacote é rara e o stream confiável resolve. Se aparecer
  latência por retransmissão: um stream por quadro com prazo (cancela o
  atrasado + `RequestKeyframe`) ou *intra-refresh*. Fica pra depois da medição.
- No viewer: decodificar e apresentar **sempre o quadro mais novo**;
  swapchain com *waitable object* (1 quadro na fila).

### 10.5 Por que UDP (QUIC) e não TCP — e TCP como reserva

Na LAN, TCP **funciona** (perda é rara). O QUIC ganha em:

| | TCP | QUIC (UDP) |
|---|---|---|
| Vários canais | uma fila só: um keyframe de 500 KB atrasa o clique e a tecla que vêm atrás (*head-of-line*); contornar = várias conexões TCP | streams independentes na mesma conexão |
| Posição do cursor | confiável e em ordem (posição velha atrasa a nova) | datagrama: chega o mais novo, o perdido não importa |
| Fila escondida | buffer de envio do kernel acumula quadros sem o app saber (precisa `TCP_NODELAY` + `SO_SNDBUF` pequeno e mesmo assim é opaco) | o app vê quanto falta enviar → regra do §10.4 exata |
| Handshake | TCP + TLS: 2 idas e voltas | 1 (0 na reconexão) |
| Wi-Fi / troca de IP | conexão cai | migração de conexão (segue) |
| Firewall corporativo | quase sempre liberado | UDP às vezes bloqueado por política |
| Ferramentas (Wireshark, proxy) | maduras | boas, mas menos |

**Decisão:** QUIC como principal; **TCP + TLS como reserva** pra rede que
bloqueia UDP. O crate `net` já fica atrás de um trait; no TCP os mesmos
canais viram 2 conexões (controle+input+cursor numa, vídeo noutra) com
`TCP_NODELAY`, e a posição do cursor vai junto do controle. Mesma porta
47800 (UDP e TCP). O viewer tenta QUIC e cai pro TCP se não houver resposta
em ~2 s (ou o usuário força).

### 10.6 Escolha de codec (H.264, HEVC, AV1)

Sim, o codec é escolhido por sessão. Cada lado informa o que tem no `Hello`:

- **Host — encoders:** a detecção do NoOBS (`caps.rs`: fabricante pelo DXGI +
  1 quadro de teste em cada encoder).
- **Viewer — decoders:** perfis do D3D11 Video
  (`ID3D11VideoDevice::GetVideoDecoderProfile` / `CheckVideoDecoderFormat`:
  `H264_VLD_NOFGT`, `HEVC_VLD_MAIN`, `AV1_VLD_PROFILE0`) + software sempre
  disponível no libavcodec (`h264`, `hevc`, `libdav1d` pra AV1).

O usuário escolhe no viewer **Auto | H.264 | HEVC | AV1** (por host salvo,
ou trocando durante a sessão). No **Auto**: o melhor codec que o host
codifica **por hardware** e o viewer decodifica **por hardware**, na ordem
AV1 > HEVC > H.264; sem par HW, H.264 (o mais barato em CPU dos dois lados).
Escolha manual impossível (ex.: AV1 sem encoder) → avisa e cai no Auto.

**Troca durante a conexão** (sem desconectar): o `SessionConfig` pode ser
reenviado a qualquer momento — codec, fps, bitrate, resolução máxima ou
monitor. Fluxo:

1. viewer manda `SessionConfig { codec: Av1, .. }` pelo controle;
2. host responde `ConfigApplied { codec, w, h }` (ou `ConfigRejected { motivo }`
   e mantém o atual);
3. host fecha o encoder antigo, abre o novo e manda keyframe — o
   `VideoFrame` carrega `codec`, então o viewer recria o decoder ao ver a mudança.

A imagem congela por ~100–300 ms (tempo de abrir o encoder/decoder) e
volta; input, clipboard e arquivos não param. No viewer: menu da barra
superior → Codec → Auto/H.264/HEVC/AV1, mostrando os indisponíveis
desabilitados com o motivo.

| | H.264 | HEVC | AV1 |
|---|---|---|---|
| Banda pra mesma qualidade | 1× | ~0,7× | ~0,5–0,6× |
| Encode HW | todas as GPUs dos últimos 10 anos | quase todas | AMD RX 7000+/9000, NVIDIA RTX 40+, Intel Arc/Core Ultra |
| Decode HW | todas | quase todas | AMD RX 6000+, NVIDIA RTX 30+, Intel 11ª ger.+ |
| Decode SW | leve | médio | `dav1d`: bom até 1080p/1440p |
| Encode SW (servidor sem GPU) | `openh264`/`x264` ok | pesado | SVT-AV1: caro pra tempo real — não usar no Auto |

Nesta máquina (RX 9070 XT) o NoOBS já mediu `av1_amf` com o quadro na GPU,
então AV1 por hardware está disponível no host. Com o mesmo par de GPUs
nos dois lados, o Auto escolheria AV1.

### 10.7 Clipboard

Sincronização nos dois sentidos, ligável/desligável por host salvo (e
"só host → viewer" / "só viewer → host").

- **Detectar cópia:** `AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE`,
  numa janela oculta (no viewer e, no host, no `reflex.exe --user` —
  §10.8.2).
- **Formatos:** texto (`CF_UNICODETEXT`), HTML (`HTML Format`), RTF, imagem
  (`CF_DIBV5`/PNG). Arquivos copiados (`CF_HDROP`) passam pela
  transferência de arquivos (§10.8).
- **Sob demanda (*delayed rendering*):** quem copiou manda só
  `ClipboardOffer { seq, formatos, tamanhos }`. O outro lado põe os formatos
  no clipboard com `SetClipboardData(fmt, NULL)`; só quando alguém **cola**
  chega `WM_RENDERFORMAT` → `ClipboardRequest { seq, fmt }` num stream novo →
  `ClipboardData`. Imagem de 20 MB copiada e nunca colada não trafega.
  Texto até ~64 KB vai junto com a oferta (cola sem espera).
- **Sem eco:** cada lado ignora a mudança que ele mesmo fez (compara
  `GetClipboardSequenceNumber` com o da última escrita própria).
- **Segurança:** não sincroniza enquanto o host está na tela de login/bloqueio
  (área de trabalho Winlogon — senhas); a thread do clipboard do agente fica
  na área de trabalho `Default`. Limite de tamanho configurável.

### 10.8 Transferência de arquivos

**1ª versão: copiar e colar arquivos pelo clipboard** (como o RDP), nos dois
sentidos: Ctrl+C numa máquina, Ctrl+V no Explorer da outra. Arrastar e
soltar e o painel de arquivos ficam pra depois (§10.8.4) e reaproveitam o
mesmo protocolo.

#### 10.8.1 Como funciona no Windows

- **Quem copia:** o clipboard dele ganha `CF_HDROP` (lista de caminhos). O
  listener do §10.7 vê e manda `ClipboardOffer` com o formato **Files**
  (quantidade, tamanho total, nomes do 1º nível) — sem conteúdo.
- **Quem cola:** recebe a oferta e põe no clipboard (`OleSetClipboard`) um
  **`IDataObject` próprio** com *arquivos virtuais*:
  - `CFSTR_FILEDESCRIPTORW`: a lista de arquivos e pastas (caminho relativo,
    tamanho, data, atributos) — pedida pela rede **quando o Explorer cola**;
  - `CFSTR_FILECONTENTS` (um por arquivo, por índice): um **`IStream`** que lê
    da rede à medida que o Explorer grava no disco;
  - `IDataObjectAsyncCapability`: o Explorer faz a cópia numa thread dele,
    **com a janela de progresso e o Cancelar do próprio Windows**, sem travar
    o Explorer.
- Implementação com `#[implement]` do crate `windows` (o NoOBS já usa) — um
  crate `clipfiles` compartilhado entre o viewer e o host.
- Só **copiar** (Ctrl+X vira cópia; não apaga a origem).

#### 10.8.2 Processo do usuário no host (`reflex.exe --user`)

O agente roda como **SYSTEM**: não pode ser o dono do clipboard nem tocar em
arquivos.
- Arquivos lidos/gravados como SYSTEM ignorariam as permissões do Windows;
- o Explorer (usuário comum) teria de chamar, via COM, um `IDataObject` dentro
  de um processo SYSTEM — bloqueado/perigoso.

Por isso o serviço lança na sessão um terceiro processo, **`reflex.exe --user`**,
com o **token do usuário logado** (`WTSQueryUserToken` →
`CreateProcessAsUserW`). Ele cuida de **clipboard (texto/imagem) e
arquivos**; o agente fica só com captura e input. Sem usuário logado (tela de
login/bloqueio) esse processo não existe → clipboard e arquivos ficam
indisponíveis (o que também resolve o "não vazar senha" do §10.7).
No viewer não há esse problema: ele já roda como o usuário.

#### 10.8.3 Protocolo

Cada pedido abre o **próprio stream** QUIC (não bloqueia vídeo, input nem
outros arquivos; o Cancelar do Explorer = resetar o stream).

```rust
// no controle
ClipboardOffer { seq, formats: [.., Files { count, total_size, top_names }] }

// ao colar (streams novos, de quem cola pra quem copiou)
FileTreeRequest { seq }      → FileTree(Vec<Entry { rel_path, is_dir, size, mtime, attrs }>)
FileRead { seq, index, offset } → FileHeader { size } … blocos de 256 KB … Done
```

- **Oferta velha:** se o clipboard da origem mudou depois da oferta, os
  pedidos com `seq` antigo recebem `Gone` → o Explorer mostra erro.
- **Pastas:** a árvore inteira vem no `FileTree` (caminhos relativos); o
  Explorer recria as pastas.
- **Prioridade baixa** (`SendStream::set_priority`) e limite de banda
  opcional: o vídeo continua sendo servido primeiro (§10.4).
- **Arquivos abertos/travados** na origem: o pedido volta com erro e o
  Explorer pergunta Pular/Tentar de novo.
- No **TCP de reserva** (§10.5) arquivos vão numa conexão TCP própria.
- Opções por host: permitir copiar arquivos (sim/não, e em que sentido),
  tamanho máximo.

#### 10.8.4 Depois da 1ª versão

- **Arrastar e soltar** na janela do viewer → pasta do host.
- **Painel de arquivos** (local × remoto): navegar, enviar, baixar,
  renomear, apagar. Mensagens extras: `ListDir`, `Upload`, `Download` (com
  retomada por `offset` e BLAKE3), `Mkdir`/`Rename`/`Delete`.

---

## 11. Usuários, permissões e acesso único

Três formas de entrar numa máquina:

| Forma | Pede senha? | Pra quê |
|---|---|---|
| **Usuário cadastrado** | não — entra pela chave do viewer | quem acessa sempre (equipe, suporte) |
| **Master** | não | dono/administrador: acessa sempre, configura e gerencia usuários remotamente |
| **Convite de acesso único** | não — um código gerado na hora | "me ajuda aqui": compartilhar sem criar usuário nem passar senha |

Não existe mais "senha do host" fixa (pode voltar como opção, desligada por padrão).

### 11.1 Identidade do viewer (o que substitui a senha)

- Cada viewer (por usuário do Windows) gera na 1ª execução um par de chaves
  **Ed25519**; a privada fica protegida por DPAPI no perfil do usuário.
- **ID de acesso** = impressão digital da chave pública, em grupos curtos
  (ex.: `K7QF-2M9X-PA4D-R8TW`). É o que se passa pra quem vai cadastrar.
  Não é segredo — sem a chave privada ninguém se passa pelo dono.
- Na conexão o viewer apresenta um certificado com essa chave (**TLS
  mútuo** no QUIC/TCP); o host compara a chave com a lista de usuários.
  Nenhuma senha trafega nem fica guardada no host.

### 11.2 Níveis e permissões

Cada usuário tem um **nível** (atalho pra um conjunto de permissões) e as
permissões podem ser ajustadas uma a uma:

| Permissão | Master | Controle total | Visualização |
|---|---|---|---|
| Ver a tela | ✅ | ✅ | ✅ |
| Teclado e mouse | ✅ | ✅ | — |
| Clipboard | ✅ | ✅ | — |
| Copiar arquivos | ✅ | ✅ | — |
| Tela de login, UAC, Ctrl+Alt+Del | ✅ | ✅ | — |
| Entrar sem confirmação do usuário local | ✅ sempre | configurável | configurável |
| Ignorar "acesso desligado" na máquina | ✅ | — | — |
| Configurar a máquina remotamente | ✅ | — | — |
| Adicionar/remover usuários, gerar convites | ✅ | — | — |

**Pedir confirmação:** por usuário, o host pode exigir que alguém na frente
da máquina clique **Permitir** (com tempo limite; sem resposta = recusado).
O master nunca precisa.

**"Acesso desligado"** (um botão na aba Esta máquina pra recusar conexões
por um tempo): bloqueia todo mundo, **menos o master**.

### 11.3 Regras do master

- **Cadastro do 1º master:** só enquanto a máquina **não tem master**, um
  administrador local adiciona pela aba Esta máquina — colando o ID de
  acesso do master ou gerando um **convite de master** (o 1º viewer que entrar
  com ele vira master).
- **Remover um master:** só um master, **conectado pela API** (§11.5). A
  interface local e o pipe do serviço recusam remover ou rebaixar master,
  mesmo pra administrador local.
- **Vários masters (decidido):** um master pode promover outro usuário a
  master, sem limite. Todos têm o mesmo poder (um master pode remover
  outro). Recomendado ter pelo menos dois — se a chave do único master se
  perder (formatou o PC, perdeu o perfil), ninguém mais consegue alterar o
  master daquela máquina pela API.
- O master **pode remover a si mesmo**; se for o último, a máquina volta ao
  estado "sem master" e um administrador local pode cadastrar outro.

⚠️ **Limite honesto:** um **administrador local** sempre pode parar ou
desinstalar o serviço, ou apagar os dados dele — nenhum programa impede isso
no próprio Windows. O que dá pra garantir:
- usuário comum (não admin) não altera nada;
- admin não remove o master **pelo Reflex** (interface e pipe recusam);
- os dados ficam em `%ProgramData%\Reflex` com ACL só pra SYSTEM;
- a lista de usuários é **assinada pela chave do serviço**; edição feita
  por fora é detectada (o serviço recusa iniciar com lista adulterada e
  registra no log);
- tudo que o master faz fica no log de sessões, visível localmente.

Recomendação de transparência: a máquina **mostra** quem é o master (nome +
ID) na aba Esta máquina, e o aviso "sendo acessado por…" aparece também nas
conexões do master.

### 11.4 Convite de acesso único

Chave **Compartilhar acesso** (liga/desliga) na aba Esta máquina:

1. Escolhe o nível dos convidados (Visualização ou Controle total — nunca
   master).
2. Enquanto ligado, a tela mostra o **código atual** de 9 caracteres (ex.:
   `482-K7Q-91X`), uma contagem regressiva até o próximo, e o **link** com
   tudo junto:
   `reflex://192.168.0.15:47800/482-K7Q-91X#K7QF2M9X` (IPs, porta, código e
   impressão digital do certificado do host — sem precisar confirmar TOFU).
   Botão **Copiar** pra mandar por chat/e-mail.
3. Quem recebe abre o link (ou cola o código e o IP no viewer) e entra.
   Por padrão aparece **Permitir / Recusar** na máquina (desligável).

Regras (decididas):
- **Código rotativo:** um código novo é gerado **a cada 10 minutos**; o
  anterior, se não foi usado, deixa de valer na troca.
- **Uso único:** a 1ª conexão bem-sucedida consome o código e já aparece
  um novo na tela (pra outra pessoa, se precisar).
- **A sessão vale enquanto estiver conectada:** a troca de código não
  derruba quem já entrou. Desconectou = acabou; pra voltar precisa de um
  código novo.
- **Seguro mesmo sendo curto:** a prova do código é por **SPAKE2** amarrado
  ao TLS — o código nunca trafega, quem escuta a rede não descobre nada, e
  cada tentativa errada só testa 1 palpite. **3 erros = código trocado
  na hora.**
- **Desligar** o compartilhar para de gerar códigos (o atual morre); as
  sessões de convidados abertas continuam até desconectarem ou serem
  derrubadas pela lista de sessões (botão **Desconectar convidados**).
- Ao fim, opção **"Lembrar este usuário"** na máquina: transforma quem
  entrou pelo convite em usuário cadastrado (a chave dele já é conhecida),
  com o nível escolhido. É o jeito mais fácil de cadastrar alguém.
- Gerar convite: o usuário local logado (não precisa ser admin) ou o master
  pela API. Configurável (ex.: proibir convites na máquina).

### 11.5 API de administração (remota, só master)

Mensagens no canal de controle, aceitas só de sessão com nível master
(as mesmas que a aba Esta máquina usa localmente pelo pipe, com as
restrições do §11.3):

```rust
ListUsers                                  → Users(Vec<User { id, name, level, perms, confirm, last_seen }>)
AddUser { access_id, name, level, perms, confirm }   → Ok | Err
UpdateUser { access_id, level?, perms?, confirm? }   → Ok | Err
RemoveUser { access_id }                   → Ok | Err   // master: só por aqui
GetConfig / SetConfig { .. }               → Config | Ok   // porta, codecs, monitor virtual, clipboard/arquivos…
ListSessions / Disconnect { session_id }
GetLog { since }                           → Log(Vec<Entry>)
SetSharing { on, level } / GetInvite        → Invite { code, link, next_rotation }
```

- No viewer: **Gerenciar máquina** na barra da sessão abre a mesma aba
  "Esta máquina", só que falando com o host remoto.
- Pra script: `reflex.exe --admin 192.168.0.15 users list` (usa a chave do
  viewer; só funciona se ela for master lá).
- Toda chamada vai pro log da máquina com o ID de quem fez.

### 11.6 Cuidados

- A chave do master abre **todas** as máquinas onde ele é master: DPAPI +
  frase-senha opcional no viewer; depois, chave em hardware (Windows Hello /
  TPM, YubiKey).
- Revogar um usuário = remover a chave dele; vale na hora, inclusive
  derrubando a sessão aberta.
- Log de sessões: quem, nível, quando, de qual IP, quanto tempo, ações de
  administração.
