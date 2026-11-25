# Reflex no Linux

O Reflex roda no Linux como **viewer e host**: conecta em qualquer máquina e é
acessado. Este documento é o plano e o registro do que já foi feito.

## Onde testar

- **Alvo principal: Ubuntu 26.04 LTS** (GNOME 50, só Wayland). Mint e Pop!_OS
  são Ubuntu por baixo; o Debian 12 é da mesma família.
- **Fedora** (só Wayland) e **Mint** (X11) ficam para conferir os extremos.
- O binário é compilado contra a **glibc 2.35** (Ubuntu 22.04), como o Aylis,
  para rodar em todas as distribuições acima.
- A VM de teste é a `claude-vm` (Hyper-V, Default Switch), acessada por SSH com
  a chave `aylis_vm` (usuário `claude`, `sudo` sem senha).

## Sem X11

O Ubuntu 26.04 e o Fedora não têm mais sessão X11 no GNOME. O host nasce no
**Wayland**, por dois caminhos:

| | Como | Pede autorização na tela | Ambientes |
|---|---|---|---|
| **Mutter** (primeiro) | `org.gnome.Mutter.RemoteDesktop` e `org.gnome.Mutter.ScreenCast` pelo D-Bus da sessão: a tela num fluxo do PipeWire (com as áreas alteradas), teclado, mouse e clipboard | não | GNOME (Ubuntu, Debian, Fedora) |
| **Portal** (depois) | `xdg-desktop-portal` RemoteDesktop + ScreenCast, com o *restore token* para não pedir de novo | na 1ª vez | GNOME, KDE, Cinnamon no Wayland... |

É o Mutter que dá o acesso sem ninguém na frente da máquina, como o Reflex faz
no Windows. O X11 (XShm + XDamage + XTest) só se algum ambiente sem Wayland
pedir.

### Tela bloqueada

Com a tela bloqueada, o GNOME **recusa abrir** uma sessão nova do Mutter
(`CreateSession` responde `Session creation inhibited`; visto na VM em
24/11/2026). Se a que já estava aberta continua depois do bloqueio ainda
**não foi conferido**. A sessão também acaba quando o processo que a criou se
desliga do D-Bus. Por isso:

- o agente abre a sessão do Mutter quando o usuário entra (a tela está
  desbloqueada) e a **mantém aberta**, em vez de abrir a cada conexão;
- conectar com a tela bloqueada e sem sessão aberta (depois de reiniciar, por
  exemplo) pede o **login remoto do GDM** (`gnome-remote-desktop` no modo do
  sistema), que é a fase 5.

## O que muda, por área

| Área | Windows | Linux |
|---|---|---|
| Captura | DXGI Desktop Duplication (textura D3D11) | PipeWire do Mutter: BGRA na memória, `SPA_META_VideoDamage` (áreas alteradas) e `SPA_META_Cursor` (o cursor à parte) |
| Codificar | placa (FFmpeg: NVENC/AMF/QSV), openh264, sem perda | openh264 e sem perda pela CPU; VAAPI depois |
| Decodificar (viewer) | FFmpeg (placa ou software), sem perda | **openh264** (decodifica também) e sem perda: sem FFmpeg |
| Teclado e mouse (host) | `SendInput` | `NotifyKeyboardKeycode` / `NotifyPointerMotionAbsolute` do Mutter (sem `uinput`, sem root) |
| Teclas no protocolo | scancode PC (conjunto 1) + estendida | o mesmo no fio; tabela scancode ↔ keycode do evdev nas duas pontas |
| Janela de vídeo (viewer) | gancho Win32 pelo `HWND` (Win, Alt+Tab) | eventos de teclado e mouse do próprio Aylis |
| Clipboard | API do Windows (todos os formatos) | host: a do Mutter (`SelectionRead`/`SelectionWrite`); viewer: a do Aylis. Texto e imagem primeiro |
| Arquivos copiados | arquivos virtuais colados no Explorer | depois (sem equivalente direto) |
| Processos | serviço (sessão 0) + agente (SYSTEM) + processo do usuário | serviço do **systemd** (root) + agente **como o usuário** da sessão gráfica (precisa do D-Bus dela) |
| Canal entre processos | pipe nomeado com ACL | socket Unix com dono e permissão, conferindo o par por `SO_PEERCRED` |
| Segredos em disco | DPAPI | arquivo `0600` do root (máquina) ou do usuário |
| Pastas | `ProgramData\Reflex`, `%LOCALAPPDATA%\Reflex` | `/var/lib/reflex`, `/var/log/reflex`; `$XDG_CONFIG_HOME/reflex`, `$XDG_STATE_HOME/reflex` |
| Instalar | `--install`: serviço, firewall, monitor virtual | `--install`: unidade do systemd, `ufw` se estiver ativo, openh264 da Cisco |
| Tela de login, Ctrl+Alt+Del, monitor virtual | sim | depois (GDM; tela sem monitor por um compositor sem saída) |

## O motor do host

O motor (`host/engine.rs`) fala com a **tela** (`host/screen.rs`), que tem a
mesma interface nos dois sistemas: abrir o monitor, esperar imagem nova (com
as áreas alteradas e o cursor), ler o BGRA das áreas pedidas e, só no
Windows com placa, codificar direto da placa (NV12 no shader).

- Windows (`screen_windows.rs`): DXGI + D3D11;
- Linux (`screen_linux.rs`): o fluxo do PipeWire da sessão do Mutter
  (`media/pipewire.rs`), sempre pelo caminho da CPU.

O resto do motor (ritmo, híbrido, refino, estatísticas) é o mesmo.

## Como usar no Linux

Na sessão gráfica do usuário (GNOME no Wayland), sem root:

```
reflex --install                              # serviço do usuário no systemd
reflex --add-user <ID> <nome> master          # o 1º master; os outros, pelo --admin
reflex --status
```

O serviço (`~/.config/systemd/user/reflex.service`) roda `reflex --agent`
junto com a sessão gráfica. Os dados do host ficam em `~/.local/share/reflex`.
Com o `ufw` ligado, libere a porta 47800 (UDP e TCP).

## Fases

1. **Compilar no Linux.** O crate `windows` vai para
   `[target.'cfg(windows)'.dependencies]`; o que é só Windows fica em
   `platform/windows/`, e o Linux ganha `platform/linux/` com a mesma
   interface. O que ainda não existe no Linux responde com um erro claro
   ("ainda não disponível no Linux"), nunca finge que funcionou. Os testes
   puros rodam nas duas plataformas.
2. **Viewer no Linux.** Conectar numa máquina Windows: openh264 e sem perda
   para decodificar, a janela de vídeo pelos eventos do Aylis, a tabela de
   teclas, o clipboard de texto e imagem.
3. **Host no Linux.** Mutter + PipeWire, teclado e mouse pelo Mutter, o
   serviço do systemd e o agente na sessão do usuário, `--install`.
4. **Pacote.** O Publicar do Aylis gera o `.deb` (e o `.rpm`).
5. **Depois.** Portal (outros ambientes), clipboard completo, arquivos, tela de
   login, VAAPI, servidor sem monitor.

## Teclado no viewer

O protocolo manda a tecla **física** (scancode PC + estendida): com isso o
layout que vale é o da máquina remota (ABNT2, teclas mortas, atalhos). No
Windows a janela de vídeo lê o scancode das mensagens do Win32; no Linux, o
`KeyEvent` do Aylis traz o `scancode` do evdev (desde a correção do Aylis
depois da 0.10.1), que o `keymap` traduz pro do PC.

O teclado inteiro (Super, Alt+Tab, Alt+F4...) vai pelo `KeyboardCapture` do
Aylis, ligado na janela de vídeo só no Linux (no Windows segue o gancho
próprio). No Wayland é o *keyboard shortcuts inhibit*: o GNOME pergunta
"Permitir inibir atalhos" e, sem um `.desktop` do app, pergunta de novo a
cada abertura; Super+Esc devolve os atalhos ao GNOME.

## Registro

- **24/11/2026** — Aylis 0.10.0 pelo crates.io (o repositório é público). VM
  Ubuntu 26.04 com o Rust e os pacotes de desenvolvimento (PipeWire, D-Bus,
  clang, Wayland, os do Aylis).
- **24/11/2026 — fase 1 feita.** O Reflex compila e os testes passam no Linux
  (233) e no Windows (319). `platform/` tem `windows/` e `linux/` com os
  mesmos nomes de módulo; o que viaja entre as máquinas e não depende do
  sistema saiu dos módulos do Windows (`platform::clip`, `service_state`,
  `media::decode`, `media::caps`). O libavcodec, o DXGI e o D3D11 ficaram
  atrás de `cfg(windows)`; o relógio passou a ser o `Instant`. No Linux: o
  `--my-id` e a janela principal; os modos do lado controlado respondem que
  ainda não existem. A janela de vídeo mostra a imagem sem perda, mas ainda
  sem entrada nem H.264 (fase 2).
- **24/11/2026 — o caminho do host conferido na VM** (com a tela
  desbloqueada), por um script Python com o Gio, fora do Reflex:
  `RemoteDesktop.CreateSession` → `ScreenCast.CreateSession` com o
  `remote-desktop-session-id` → `RecordMonitor("", cursor-mode 2 = metadado)`
  → `Start` → `PipeWireStreamAdded` (nó do PipeWire; monitor `Virtual-1`,
  1920×1080). Um quadro lido do nó pelo `pipewiresrc` veio com a tela inteira,
  sem o cursor. `NotifyPointerMotionAbsolute` e `NotifyKeyboardKeycode`
  aceitos. **Nada aparece pedindo autorização**; o GNOME mostra o indicador de
  compartilhamento de tela na barra de cima enquanto a sessão existe (serve de
  aviso "sendo acessado").
- **24/11/2026 — a janela principal abre no Linux** (Wayland, OpenGL por
  software na VM). Detalhes: a opção "Escala do Windows" aparece com esse
  nome; o dock mostra um ícone genérico (falta o `.desktop` com o ícone).
- **24/11/2026 — Linux controlando Windows, entre as VMs.** O probe do Ubuntu
  na VM Windows: 1920×1080 sem perda, 17 quadros/s, tecla→tela 35 ms
  (mediana). A janela de vídeo do Linux mostra o Windows com o cursor remoto.
- **24/11/2026 — o lado controlado no Linux (fase 3) funcionando.** Mutter
  pelo zbus (`platform/linux/mutter.rs`), PipeWire com as áreas alteradas e o
  cursor no metadado (`media/pipewire.rs`; o metadado do cursor só vem se a
  faixa de tamanho pedida cobre a que o Mutter oferece), input pelo Mutter
  com a tabela de teclas (`keymap`), o agente pelo systemd do usuário
  (`install_linux.rs`). Do Windows: conectou como master, viu a tela do
  Ubuntu, a tecla Windows abriu a visão geral do GNOME e o arrasto moveu o
  cursor. Com a build de depuração o sem perda ficava em ~100 ms por quadro;
  a instalada é a otimizada.
- **24/11/2026 — mouse e teclado na janela de vídeo do Linux (fase 2).** Pelos
  eventos do Aylis (`on_pointer_*`, `on_wheel` no `FrameView`; `on_key_*` no
  form com `key_preview`) e o `scancode` físico do `KeyEvent`. Testado entre
  as VMs simulando quem usa o Ubuntu (mouse e teclado de verdade pelo
  Mutter): clicou na pesquisa do Windows, digitou "bloco", o Enter abriu o
  Bloco de Notas, e a tecla do "ç" do ABNT2 chegou como "ç".
- **24/11/2026 — a janela de vídeo do Linux com as APIs novas do Aylis.**
  O `KeyboardCapture` (o Super abriu o Iniciar do Windows remoto e não a
  visão geral do GNOME), a escala da janela (`form_scale`/`on_scale_change`),
  as travas (`lock_keys`, com o Scroll Lock), o cursor remoto como cursor
  daqui no "só o remoto" (`set_cursor_image`: o I-beam do Windows apareceu
  sobre o texto, sem cursor duplo) e a resolução "igual à janela" pelo
  `on_resize_end`. O monitor da janela o Wayland não diz: vale o principal.
  Não funcionou: ajustar a janela ao tamanho da tela remota (o
  `client_width`/`client_height` com a janela aberta não muda o tamanho no
  GNOME Wayland; passado à sessão do Aylis).
- **Pendente:** o H.264 no Linux (o openh264 pra Linux: decodificar no viewer
  e codificar no host — hoje só o sem perda), o clipboard (o GNOME não tem o
  *data-control* do Wayland: o do Aylis cai no X11), a tela bloqueada (a
  sessão aberta antes do bloqueio continua?), a janela do tamanho da tela
  remota (depende do Aylis), a escala fracionária (não testada) e o ícone do
  `.desktop` (que também faria o GNOME lembrar a permissão dos atalhos).
