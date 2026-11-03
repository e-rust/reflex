# Reflex

Controle remoto de tela para Windows, em Rust, para a rede local. Pensado para
administrar máquinas e servidores — inclusive máquinas fracas, sem placa de
vídeo e sem monitor. Um único `reflex.exe` faz tudo: janela do viewer,
serviço do host e linha de comando.

O levantamento completo (decisões, arquitetura, protocolo) está em
[docs/levantamento.md](docs/levantamento.md).

## O que faz

- **Vídeo**: captura DXGI, H.264/HEVC/AV1 por placa de vídeo ou x264 pela
  CPU; refino da imagem com a tela parada e cor completa (4:4:4) opcional.
  Em máquina sem placa (VM, servidor) só o que mudou na tela é lido e
  convertido.
- **Controle total**: teclado (inclusive Win, Alt+Tab e Ctrl+Alt+Del), mouse,
  tela de login, bloqueio e UAC; Caps/Num/Scroll Lock sincronizados.
- **Sem monitor**: instala e liga um monitor virtual
  ([Virtual Display Driver](third_party/vdd/README.md)) quando a máquina não
  tem tela ativa.
- **Clipboard** com todos os formatos (texto, HTML, RTF, imagens, formatos
  próprios dos programas) e **cópia de arquivos e pastas** colando direto no
  Explorer, com a barra de progresso do Windows, nos dois sentidos.
- **Várias conexões ao mesmo tempo** compartilhando uma captura e um encoder.
- **Reconexão automática** quando a conexão cai (troca de sessão do Windows,
  reinício do serviço).
- **Segurança**: QUIC (com TCP de reserva) sempre em TLS 1.3 mútuo;
  usuários cadastrados por ID de acesso (níveis master, controle total e só
  ver) ou convite por senha (SPAKE2); administração remota pelos masters.
- **Janela** do tamanho da tela remota (ou a tela remota do tamanho da
  janela), cursor remoto sem atraso, opções de qualidade, cor, cursor e
  clipboard.

## Arquitetura

| Crate | Papel |
|---|---|
| `reflex-proto` | mensagens e enquadramento do protocolo |
| `reflex-auth` | identidade, usuários, master, convites (SPAKE2) |
| `reflex-net` | transporte QUIC + TCP/TLS mútuo |
| `reflex-media` | captura DXGI, conversões, codec (libavcodec) |
| `reflex-platform` | input, áreas de trabalho, clipboard, arquivos virtuais, pipes, monitor |
| `reflex-host` | aceitar conexões, motor de captura compartilhado, ponte com o usuário |
| `reflex-viewer` | conexão do viewer, decodificação |
| `reflex-ui` | interface (winsafe) e janela de vídeo |
| `reflex` (`app/`) | o executável: serviço, agente, processo do usuário, CLI |

No host, o **serviço** (sessão 0) mantém um **agente** como SYSTEM na sessão
de console — é ele que captura, injeta input e vê a tela de login — e, com
alguém logado, o **processo do usuário**, que cuida do clipboard e dos
arquivos com as permissões de quem está logado.

## Compilar

Requer Windows 10/11 x64 e Rust estável.

```
.\scripts\empacotar.ps1
```

Antes, ponha as DLLs do FFmpeg em `third_party\ffmpeg`: elas não estão no
repositório (o `libx264` é GPL) — ver
[third_party/ffmpeg/README.md](third_party/ffmpeg/README.md).

O script compila e monta **`dist\`** com só o que vai pra outra máquina: o
`reflex.exe` e as DLLs de `third_party\ffmpeg`. Copie a pasta e rode
`reflex.exe --install` como administrador. Pra usar nesta máquina, abra o de
`dist\` (é uma cópia: aberto, não trava o próximo build).

## Uso

```
reflex                                   janela principal
reflex --connect <endereço>              conecta direto (ip ou nome, porta opcional; padrão 47800)
reflex --my-id                           ID de acesso deste viewer
reflex --install / --uninstall           serviço, firewall, monitor virtual (admin)
reflex --add-user <ID> <nome> <nível>    master | full | view (admin)
reflex --status                          estado, ID da máquina, endereços, usuários (admin)
reflex --admin <endereço> ...            administração remota (master)
reflex --probe <endereço> [s] [png] ...  viewer sem janela, pra testes e medidas
reflex --service | --agent | --user      iniciados pelo próprio Reflex
```

Logs: `C:\ProgramData\Reflex\logs` (serviço, agente) e
`%LOCALAPPDATA%\Reflex\logs` (interface, processo do usuário). Com uma conexão
aberta, o host manda a cada 5 s os tempos do motor e o uso de CPU, que vão
pro log da interface (`host: motor …`).

## Testes

```
cargo test --workspace
```

Os que precisam de GPU, monitor, FFmpeg ou do clipboard de verdade ficam
marcados `#[ignore]`:

```
cargo test -p reflex-media --test pipeline -- --ignored --nocapture
cargo test -p reflex-viewer --test end_to_end -- --ignored --nocapture
```

### Numa VM Hyper-V

Num PowerShell **como administrador**, na pasta do projeto:

```
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force
. .\scripts\vm-conectar.ps1                     # pede a senha da VM uma vez
.\scripts\empacotar.ps1                         # monta dist\
& .\scripts\testar-vm.ps1 -ViewerId <seu ID>    # copia, instala e cadastra o master
```

No *Default Switch* a VM muda de IP a cada reinício do PC: conecte pelo nome
do computador com `.mshome.net` (ex.: `DESKTOP-XXXX.mshome.net`).

## Licença

[MIT](LICENSE) para o código do Reflex.

As dependências binárias em `third_party\` têm licenças próprias: o
[Virtual Display Driver](third_party/vdd/LICENSE) é MIT; o FFmpeg é
LGPL/GPL e o **`libx264` é GPL** — distribuir o Reflex com ele obriga a seguir
a GPL. Por isso as DLLs do FFmpeg não estão no repositório, e a troca do x264
pelo `openh264` (BSD) está nas [pendências](docs/pendencias.md).

Pendências do projeto: [docs/pendencias.md](docs/pendencias.md).
