# Híbrido imagem + vídeo

Mandar como **imagem sem perda** o que muda pouco e **vídeo** só onde a banda
não comporta o sem perda. Decidido em 15/11/2026 a partir das medidas abaixo
(ver também [pendencias.md](pendencias.md)).

## Medidas (VM Hyper-V, 1366×768, 1 núcleo, placa de software)

`reflex --probe <vm> 22 <cenário> --medir-imagem`: o host mede, nos mesmos
quadros, o H.264 de sempre (openh264) e os retângulos alterados compactados
sem perda. Valores estáveis (depois dos 5 primeiros segundos); o tempo é por
quadro.

| Cenário | Quadros/s | H.264 | lz4 | zstd | zstd do delta |
|---|---|---|---|---|---|
| Digitação (busca do Iniciar) | 19 | 146 KB/s, 8,7 ms | 482 KB/s, 0,3 ms | **292 KB/s, 0,4 ms** | 534 KB/s, 0,5 ms |
| Arrasto contínuo | 29 | 14–23 KB/s, 5,7 ms | 2.150 KB/s, 0,2 ms | 1.150 KB/s, 0,3 ms | **270 KB/s, 0,1 ms** |
| Menu Iniciar abrindo/fechando | 20 | 53 KB/s, 6,9 ms | 4.800 KB/s, 1,5 ms | 3.600 KB/s, 2,4 ms | **1.900 KB/s, 1,3 ms** |

- **CPU:** sem perda custa de 3 a 20 vezes menos que o H.264 na máquina fraca.
- **Banda:** de 2 vezes (digitação) a 35 vezes (animação) a do H.264; no pior
  caso ~3,6 MB/s (~29 Mbit/s): cabe na rede cabeada, aperta no Wi-Fi ruim, não
  serve pela internet.
- **zstd ou delta:** o delta (XOR com o quadro anterior) ganha quando pouco
  muda dentro da área; o zstd direto, quando a área é redesenhada inteira.
  Escolher por quadro o menor dos dois.
- **Rolagem:** os três cenários deram 0% de área "só movida"; falta medir com
  rolagem de verdade (se a DXGI manda *move rects* nesta VM).

## Desenho

### Etapa 1 — codec "sem perda"

- `Codec::Lossless` no protocolo (versão 9). O host sempre sabe codificar e o
  viewer sempre sabe decodificar (é CPU pura, sem DLL).
- **Encoder** (`media/lossless_codec.rs`), no caminho de CPU que já existe (o
  BGRA lido só nas regiões alteradas): junta os retângulos, compacta com zstd
  nível 1, direto e como XOR do quadro anterior, e manda o menor. Keyframe
  (início, quem entrou, pedido): a tela inteira, sem delta.
- **Pacote:** versão, flags (keyframe, delta), largura, altura, a lista de
  retângulos e um bloco zstd com os pixels deles, em ordem.
- **Decoder:** guarda a tela inteira e aplica os retângulos (cópia ou XOR);
  antes do 1º keyframe, ignora.
- **Escolha automática:** sem encoder de placa no host (VM, servidor), sem
  perda; com placa, o vídeo de placa como hoje. Pedido de cor 4:4:4: sem perda
  (é 4:4:4 por natureza), no lugar do x264.
- Com sem perda não há refino (a tela parada já é exata).
- O stream de vídeo é confiável e em ordem (QUIC/TCP), então o delta não se
  perde no caminho.

#### Resultado da etapa 1 (VM, 15/11/2026)

`reflex --probe <vm> 17 <cenário> --codec=sem-perda|h264`, valores estáveis:

| Cenário | Codec | Quadros/s | Banda | Codec por quadro | CPU da VM | Tecla→tela p50 / p90 / máx |
|---|---|---|---|---|---|---|
| Digitação | sem perda | 22 | 390 KB/s | 2,1 ms | 54% | 39 / 60 / 83 ms |
| Digitação | H.264 | 22 | 193 KB/s | 10,5 ms | 58% | 33 / 73 / 100 ms |
| Arrasto | sem perda | 29 | 274 KB/s | 0,4 ms | ~25% | — |
| Arrasto | H.264 | 29 | 14–23 KB/s | 5,7 ms | ~26% | — |
| Menu Iniciar | sem perda | 14 (todos imagem nova) | 1.200–1.600 KB/s | 3,9 ms | 17% | — |
| Menu Iniciar | H.264 | 20 (29 de refino a cada 5 s) | 50 KB/s | 6,6 ms | 21% | — |

- O codec por quadro cai de 3 a 15 vezes, mas **nesta VM o gargalo é a
  captura** (o Windows desenhando a tela por software: 10–16 ms por imagem):
  os quadros/s ficam iguais e a CPU da máquina cai pouco (de 2 a 5 pontos).
- O texto chega exato e a cor completa; o pior caso de latência (p90, máx)
  melhora na digitação.
- A banda sobe de 2 vezes (digitação) a 30 vezes (animação); no pior caso
  ~1,6 MB/s a 1366×768.
- O menu Iniciar manda só imagens novas (o H.264 completa com quadros de
  refino); o número de imagens novas é o mesmo.

### Etapa 2 — troca automática pela banda

- O motor mede quanto a fila de envio aguenta (o `InFlight` que já existe dá
  o ritmo). Se o sem perda passa do orçamento por alguns quadros, troca pro
  H.264; quando a tela acalma, volta pro sem perda com um keyframe sem perda
  (a imagem do H.264 tem perda, o delta não serve em cima dela).
- O viewer já troca de decoder quando o codec do quadro muda.

### Etapa 3 — rolagem e repetição

- *Move rects* da DXGI viram "copie esta área pra lá", sem pixel.
- Cache de pedaços repetidos (ícones, botões) por hash.
