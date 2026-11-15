# runtime

O runtime de desenvolvimento (o `output` do projeto do Aylis): a IDE põe aqui o
`reflex.exe` que compila, ao lado das DLLs que ele carrega em tempo de
execução. Só este README vai pro repositório; o resto, `scripts\obter-dependencias.ps1`
baixa. O Publicar leva as DLLs do FFmpeg junto do exe (`publish.windows.files`).

| Arquivo | O que é |
|---|---|
| `avcodec-61`, `avutil-59`, `swresample-5`, `swscale-8` | FFmpeg 7.1, build **LGPL** *shared* |
| `openh264-2.7.0-win64.dll` | openh264 2.7.0 da Cisco (BSD-2-Clause) |

## FFmpeg

O `reflex.exe` carrega o FFmpeg (`rust/src/media/ffi`): os encoders de placa de
vídeo (AMF, NVENC, Media Foundation) e todos os decoders (H.264, HEVC; AV1 pela
placa). O script baixa a build LGPL do
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (sem x264 e sem nada
GPL), numa versão fixa com o hash conferido.

**Licença:** o FFmpeg é LGPL 2.1+. Por ser carregado como DLL, o Reflex pode
ser MIT; quem distribui as DLLs tem de oferecer o código-fonte do FFmpeg (o
BtbN publica o de cada build) e deixar trocá-las.

Quem quiser cor completa (4:4:4) pode usar uma build GPL com o `libx264`: o
Reflex detecta e usa o x264 só pra isso. Distribuir o Reflex com essa build
obriga a seguir a GPL.

## openh264

O H.264 pela CPU (máquinas sem placa de vídeo: VMs, servidores) é o openh264 da
Cisco, carregado pelo crate [`openh264`](https://crates.io/crates/openh264)
(recurso `libloading`), que confere o SHA-256 contra as versões conhecidas.

**A DLL não vai na publicação.** A Cisco paga a licença de patente do H.264
pelo binário dela **só quando quem usa o baixa da Cisco**
(<https://www.openh264.org/faq.html>). Por isso, numa instalação, o
`reflex.exe --install` baixa de `http://ciscobinary.openh264.org/` pra pasta do
exe e confere o hash; aqui, `reflex --baixar-openh264 runtime` (ou o script).

> OpenH264 Video Codec provided by Cisco Systems, Inc.
