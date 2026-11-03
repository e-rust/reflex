# FFmpeg + x264 + runtime do C (DLLs)

O `reflex.exe` carrega o FFmpeg em tempo de execução (`crates/media/src/ffi`):
codificação/decodificação H.264, HEVC e AV1. O Windows procura as DLLs na
pasta do exe, então elas vão junto no pacote (`scripts\empacotar.ps1` →
`dist\`). Em desenvolvimento o `app\build.rs` as põe ao lado do exe em
`target\` (hard link) e os testes as acham aqui.

| DLL | O que é |
|---|---|
| `avcodec-61`, `avutil-59`, `swscale-8`, `swresample-5`, `zlib` | FFmpeg 7.x (mesma build do NoOBS) |
| `libx264-164` | encoder H.264 por CPU (máquinas sem GPU) |
| `vcruntime140` | runtime do C da Microsoft; as DLLs do FFmpeg precisam dele e um Windows limpo não tem |

**As DLLs não estão no repositório** (ver o `.gitignore`): copie pra esta
pasta uma build *shared* do FFmpeg 7.x para Windows x64 com o `libx264`
(os nomes da tabela acima — a mesma build do NoOBS) e o `vcruntime140.dll`
do *Visual C++ Redistributable*. Sem elas o Reflex compila, mas não codifica
nem decodifica vídeo.

⚠️ **Licença:** o `libx264` é GPL. Distribuir o Reflex com ele obriga a seguir
a GPL (inclusive oferecer o código-fonte). Alternativa sem isso: `openh264`
(BSD; o binário da Cisco cobre a patente) — ver `docs/levantamento.md` §4.2.
A troca está planejada em `docs/pendencias.md`.
