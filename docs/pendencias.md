# Reflex — pendências

O que falta no projeto como um todo, por área. Cada item diz o que é, por que
importa e, quando já se sabe, o caminho. Referências como §4.2 apontam pro
[levantamento](levantamento.md).

Prioridade: **Alta** = bloqueia publicar ou usar de verdade; **Média** =
incomoda no uso; **Baixa** = melhoria ou depois da 1ª versão.

## Licença e distribuição

### Trocar o x264 pelo openh264 — Alta

O encode por CPU (máquinas sem placa de vídeo: VMs, servidores) usa o
`libx264` pelo FFmpeg. O x264 é **GPL**: distribuir o Reflex com ele obriga o
projeto inteiro a seguir a GPL, o que conflita com a licença MIT. Por isso as
DLLs do FFmpeg saíram do repositório (`.gitignore`) até a troca.

O caminho que o levantamento já indica (§4.2, §6):

- **`openh264`** (Cisco, BSD) para H.264 por CPU. O binário que a Cisco
  distribui cobre a licença de patente do H.264; compilar o openh264 por
  conta própria não tem essa cobertura.
- **FFmpeg em build LGPL** (sem `--enable-gpl`, sem x264) para o resto:
  encoders de placa de vídeo (AMF, NVENC, QSV) e os decoders. Alternativa:
  usar o openh264 direto pelo crate `openh264` e tirar o FFmpeg do caminho
  de CPU.
- Ajustar o que hoje depende do x264:
  - afinação de tempo real (`low_latency_options`, preset `ultrafast`, VBV,
    `refine`): o openh264 tem outros parâmetros (`RC_MODE`, `iTargetBitrate`,
    `bEnableFrameSkip`, `iMultipleThreadIdc`); medir de novo na VM de 1 núcleo;
  - **cor completa (4:4:4)**: o openh264 só faz 4:2:0 (perfil Baseline/High
    sem 4:4:4). Opções: deixar o 4:4:4 só para quem tiver placa que suporte
    ou para builds com x264 (opcional, por conta do usuário);
  - keyframe sob demanda e a reabertura do encoder (`can_force_key`).
- Testes: `x264_from_cpu_bgra_preserves_colors_and_is_fast`,
  `full_chroma_keeps_one_pixel_color_stripes` e
  `only_the_changed_region_is_read_and_converted` passam a cobrir o encoder
  novo.
- Documentar de onde baixar o binário da Cisco e o FFmpeg LGPL (ou um
  script que baixe e confira o hash).

### DLLs fora do repositório — Alta

Até a troca acima, quem clona não tem as DLLs (`third_party/ffmpeg`).
Precisa de um jeito reprodutível de obtê-las: um script que baixe versões
fixas, confira o hash e as ponha na pasta (ou um *release* com o pacote
pronto). O mesmo vale para o `vcruntime140.dll` (redistribuível da
Microsoft).

### Assinatura do driver e do executável — Média

O monitor virtual (MttVDD) vem assinado pela SignPath; a VM de teste está em
"Modo de Teste" do Windows, então ainda não foi confirmado que ele instala
numa máquina com a assinatura de drivers exigida normalmente (§4.2). O
`reflex.exe` não é assinado (SmartScreen e antivírus reclamam).

## Conexão e sessões

### Conexão sobrevive à troca de sessão do Windows — Alta

Ao sair da conta e entrar de novo, nessa VM o Windows cria uma sessão de
console nova (o log mostrou 1 → 2 → 3 → 4). O agente vive na sessão, então
o serviço lança outro e a conexão cai; o viewer reconecta sozinho em ~1 s.
Caminho: o **serviço** (sessão 0) passa a segurar a conexão de rede e o
agente de cada sessão só captura/codifica/injeta, falando com o serviço por
pipe. Na troca, só a imagem congela por um instante.

### Clipboard da VM para cá parou depois de um login em sessão nova — Alta

Observado depois de sair e entrar de novo na conta: o que é copiado no viewer
chega à VM, mas o que é copiado na VM não volta. O processo do usuário
conectou ao agente (log "ligado ao agente"), então a suspeita é o filtro de
área de trabalho segura (`input_desktop_name()` devolvendo nada) ou a ponte
do agente (`UserBridge`) depois do reinício. Não investigado.

### Confirmação "Permitir/Recusar" no host — Média

Usuários com nível que exige confirmação (e convites) dependem de alguém na
máquina clicar Permitir. Hoje o agente **sempre recusa** (`agent.rs`,
`confirm`). Falta o processo do usuário mostrar o aviso e responder pelo
pipe; sem ninguém logado, continua recusando.

### Aviso "sendo acessado" no host — Média

Quem está na máquina não sabe que há alguém conectado. O processo do
usuário deveria mostrar um aviso discreto (quem, desde quando) com a opção
de desconectar.

### Sessões RDP em servidor — Baixa

Quando o admin desconecta do RDP, a sessão sai do console e o agente perde a
tela. Opcional: devolver a sessão ao console ao desconectar
(`WTSConnectSession`/`tscon`, §4.2). Testar Windows Server 2019/2022/2025.

## Várias conexões

- **Cada conexão com o próprio monitor e configuração — Média.** O motor é
  um só por máquina: vale o último pedido (resolução, qualidade, cor) e todos
  veem o mesmo monitor. Um motor por monitor resolveria o caso de dois
  viewers olhando telas diferentes.
- **Cópia num viewer não chega aos outros viewers — Baixa.** O que um viewer
  copia vai pro clipboard do host, mas o host não repassa aos outros viewers
  conectados (o processo do usuário ignora o eco das próprias escritas).
- **A mais lenta dita o ritmo — Baixa.** Um viewer numa rede ruim segura o
  quadro de todos. Alternativa: descartar quadros só pra ele até o próximo
  keyframe.

## Segurança

- **Conferir a identidade do host no viewer — Alta.** A janela aceita
  qualquer certificado (`connect.rs`: `ServerCheck` que sempre aceita, também
  na reconexão). Falta a lista salva de hosts com a impressão digital (TOFU
  de verdade: avisar se mudou), que o `Hosts::verifier` já sabe fazer.
- **Registro de sessões — Média.** `sessions list` da administração remota
  não tem implementação (`NoSessions`); falta o log de quem conectou, nível,
  IP, duração e ações de administração (§11.6).
- **Revogar derruba a sessão aberta — Média.** Remover um usuário vale pra
  próxima conexão; conferir que derruba a que está aberta (§11.6).
- **Proteção da chave do master — Baixa.** Hoje só DPAPI; frase-senha
  opcional e, depois, chave em hardware (Windows Hello/TPM) (§11.6).

## Vídeo e desempenho

- **Máquinas de 1 núcleo sob carga — Média.** Com o menu Iniciar aberto a VM
  vai a 100% de CPU e cai para ~5 quadros/s. Opções: limitar a resolução
  remota em máquina fraca (o viewer amplia), ajustar o teto fixo de 30
  quadros/s, prioridade das threads.
- **Bitrate adaptativo — Média.** Bitrate fixo por sessão; falta adaptar
  à rede (perda, RTT) (fase 4 do levantamento).
- **Estatísticas na tela — Baixa.** As medidas do host vão pro log; mostrar
  quadros/s, latência e bitrate na janela.
- **Apresentação por D3D11 — Baixa.** A janela pinta com `StretchDIBits`; uma
  swapchain D3D11 tiraria a conversão NV12→BGRA da CPU no viewer e a cópia
  por quadro (hoje ~8 ms a 2560×1600).
- **Buffers reaproveitados no viewer — Baixa.** Cada quadro decodificado
  aloca um buffer novo (16 MB a 2560×1600).
- **Resolução inicial do monitor virtual — Baixa.** O modo de 720p não é
  aplicado porque o Windows lembra o último modo do monitor.

## Interface

- **Escolher o monitor — Média.** O viewer sempre pede o monitor 0; falta
  escolher entre os monitores do host (fase 5).
- **Cursor na escala "Windows" — Baixa.** Com a imagem ampliada, o cursor
  remoto (desenhado pelo Windows daqui) fica no tamanho original.
- **Ao reconectar, reenviar o clipboard atual — Baixa.** Depois de uma
  reconexão, o host não recebe o que já estava copiado aqui.
- **Instalador, ícone e modo escuro — Média.** Hoje é `reflex.exe --install`
  na mão; falta instalador, ícone próprio e tema escuro.
- **Viewer em outros sistemas — Baixa.** Decisão em aberto (§9.6).

## Arquivos e mídia (depois da 1ª versão)

- **Arrastar e soltar** arquivos na janela do viewer → pasta do host (§10.8.4).
- **Painel de arquivos** local × remoto, com retomada e conferência (§10.8.4).
- **Áudio** (Opus) (fase 7).

## Internet — Baixa

Hoje é só LAN. Pra usar pela internet: servidor de encontro (*rendezvous*) e
*relay* (fase 8, §6.1).

## Problemas a confirmar

- **A Lupa que abria sozinha na VM.** Causa provável: tecla Windows presa no
  host (o "+" virava Win+"+"). As teclas presas agora são soltas ao fim da
  sessão e no início do injetor; confirmar que não voltou a acontecer.

## Projeto

- **CI — Média.** Compilar, `cargo clippy` e `cargo test --workspace` a cada
  push. Os testes com GPU, monitor ou clipboard de verdade (`#[ignore]`)
  continuam manuais.
- **Testes em Windows Server e em máquinas físicas sem GPU — Média.**
