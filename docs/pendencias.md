# Reflex — pendências

O que falta no projeto como um todo, por área. Cada item diz o que é, por que
importa e, quando já se sabe, o caminho. Referências como §4.2 apontam pro
[levantamento](levantamento.md).

Prioridade: **Alta** = bloqueia publicar ou usar de verdade; **Média** =
incomoda no uso; **Baixa** = melhoria ou depois da 1ª versão.

## Licença e distribuição

### ~~Trocar o x264 pelo openh264~~ — feito

O H.264 pela CPU é o openh264 da Cisco (`rust/src/media/openh264enc.rs`),
baixado da Cisco pelo `--install` (condição da licença de patente) com o
hash conferido; o FFmpeg é a build LGPL (só placa de vídeo e decoders). As
DLLs não ficam no repositório: `scripts/obter-dependencias.ps1` baixa versões
fixas e confere o hash. O `vcruntime140.dll` não é mais preciso (CRT
estático; a build LGPL do BtbN não depende dele).

O que sobrou:

- **Cor completa (4:4:4) pela CPU** — Baixa: o openh264 só faz 4:2:0. Hoje o
  4:4:4 só existe com uma build GPL do FFmpeg (com x264) posta pelo usuário;
  sem ela o pedido cai em 4:2:0. Caminho: 4:4:4 por placa de vídeo (HEVC
  RExt no NVENC) ou aceitar 4:2:0 com o refino.
- **Afinação do openh264** — Média: está com taxa constante sem pular
  quadro (o openh264 avisa que assim não segura a taxa à risca) e sem
  detecção de troca de cena. Medir na VM de 1 núcleo e comparar com o x264
  (bits por quadro, tempo de encode, nitidez do texto).

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

## Transporte da imagem (decidido em 15/11/2026)

Três frentes, nesta ordem:

### 1. Híbrido imagem + vídeo — Alta (em andamento)

Mandar como **imagem sem perda** o que muda pouco (texto, janelas, menus — o
grosso da administração remota) e **vídeo** só nas regiões que mudam o tempo
todo (um vídeo tocando, uma animação), como o RDP faz.

- O DXGI já entrega os retângulos alterados (*dirty rects*) e os que só
  mudaram de lugar (*move rects*): a rolagem e o arrasto de janela viram
  "copie esta área para lá", sem pixel nenhum.
- Compactação livre de patente e de licença: zstd ou lz4 (BSD), QOI (MIT);
  JPEG só se a área for foto. Cache de pedaços repetidos (ícones, botões).
- Sem perda, a cor completa (4:4:4) vem de graça — hoje só existe com o x264
  (GPL) — e a tela parada já fica perfeita (o refino com H.264 deixa de ser
  preciso).
- O motor decide por região, pela frequência de mudança.
- Antes: **medir** bytes e tempo por quadro (digitação, rolagem, arrastar
  janela, vídeo) com zstd/lz4 contra o H.264 de hoje, aqui e na VM de 1 núcleo.
- Limite: movimento contínuo em tela cheia sem perda passa de dezenas de MB/s;
  serve na rede cabeada, sofre no Wi-Fi e não serve pela internet — por isso
  o vídeo continua para essas regiões.

### 2. Tirar o FFmpeg — Média

Hoje ele dá os encoders das placas, todos os decoders e o swscale (66 MB do
`avcodec` no pacote, quase tudo). Sem ele:

- encoders e decoders das placas pelo **Media Foundation** do Windows (o
  H.264 já vem licenciado pelo Windows e pelos fabricantes; HEVC e AV1 pelas
  extensões da Store);
- H.264 pela CPU, codificar **e decodificar**, pelo **openh264** da Cisco (a
  patente coberta pelo binário dela);
- AV1 sem placa no viewer pelo **dav1d** (BSD);
- some a dúvida de patente do decoder por software do FFmpeg.

Caminho intermediário, se a troca demorar: um FFmpeg LGPL mínimo, só com os
codecs que o Reflex usa (poucos MB).

### 3. AV1 e AV2 — Baixa

- **AV1** (livre de royalties) já é usado quando os dois lados têm placa;
  vale preferi-lo sempre que houver encoder AV1 no host. Não serve para host
  sem placa: codificar AV1 pela CPU pesa várias vezes o openh264.
- **AV2:** especificação recém-saída, sem placa que codifique/decodifique e
  sem encoder em tempo real. Rever quando houver suporte em hardware.

## Vídeo e desempenho

- **Máquinas de 1 núcleo sob carga — Média.** Com o menu Iniciar aberto a VM
  vai a 100% de CPU e cai para ~5 quadros/s. Opções: limitar a resolução
  remota em máquina fraca (o viewer amplia), ajustar o teto fixo de 30
  quadros/s, prioridade das threads.
- **Bitrate adaptativo — Média.** Bitrate fixo por sessão; falta adaptar
  à rede (perda, RTT) (fase 4 do levantamento).
- **Estatísticas na tela — Baixa.** As medidas do host vão pro log; mostrar
  quadros/s, latência e bitrate na janela.
- **Apresentação por D3D11 — Baixa.** A janela pinta o BGRA no `FrameView` do
  Aylis; levar o NV12 direto pra placa tiraria a conversão NV12→BGRA da CPU no
  viewer (hoje ~8 ms a 2560×1600) e a cópia por quadro.
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
  na mão; falta instalador (o Publicar do Aylis gera um), ícone próprio e
  tema escuro (o Aylis tem temas).
- **`--connect` sem a janela principal — Baixa.** Com a interface no Aylis,
  o `reflex --connect` abre a janela principal (já conectando) junto com a de
  vídeo; antes abria só o vídeo.
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

- **CI — Média.** Compilar, `cargo clippy` e `cargo test` (em `rust\`) a cada
  push. Os testes com GPU, monitor ou clipboard de verdade (`#[ignore]`)
  continuam manuais.
- **Testes em Windows Server e em máquinas físicas sem GPU — Média.**
