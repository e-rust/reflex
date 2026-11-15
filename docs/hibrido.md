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

### Etapa 2 — troca automática pela banda (feita)

`host/hybrid.rs` (pura, com testes) e o motor:

- **Fila curta no transporte** (`net/quic.rs`): o padrão do quinn (1,25 MB por
  stream) escondia a rede lenta — o quadro "saía" na hora e ficava na fila;
  com o viewer lendo a 100 KB/s a imagem chegava 12 s atrasada. Com 256 KB a
  escrita trava logo e o motor descarta quadros em vez de acumular.
- **Sem perda → vídeo:** muitos quadros atrasados (mais de 100 ms pra sair),
  um travado (mais de 1 s), ou um quadro sem perda que levaria mais de 250 ms
  no link medido (nem sai).
- **Vídeo → sem perda:** o motor estima quanto o sem perda ocuparia (zstd
  direto e do delta, o menor); cabendo em metade da banda medida, volta.
- **Começo:** em H.264; sem a rede segurar nenhum quadro em 1 s, sem perda.
- Só no automático (sem perda escolhido por falta de encoder de placa, e os
  dois lados com H.264); pedido explícito fica no codec pedido.
- `reflex --probe --banda=KB/s` lê o vídeo devagar (testa sem rede lenta).

#### Resultado da etapa 2 (VM, 15/11/2026)

| Cenário | O que acontece | Captura→chegada p50 / p90 |
|---|---|---|
| Arrasto, rede rápida | H.264 por 1 s, depois sem perda | 1,4 / 4,7 ms |
| Digitação, rede rápida | idem; tecla→tela p50 33 ms, p90 61 ms | — |
| Menu Iniciar a 300 KB/s | sem perda, um keyframe lento (4 s), H.264 de vez | 31 / 367 ms |
| Arrasto a 100 KB/s | idem, mas o keyframe sem perda leva 12 s | 859 ms / 12 s |

**Falta (etapa 2b):** o keyframe sem perda (a tela inteira, centenas de KB a
mais de 1 MB) é o 1º quadro grande da sessão e o único jeito de descobrir que
o link é lento — os quadros de H.264 cabem na janela de 256 KB e "saem" na
hora mesmo num link lento. Num link lento ele custa segundos uma vez. Saída:
mandar esse keyframe em pedaços menores que a janela (sobre a imagem do
H.264, que o viewer já tem), parando no 1º pedaço que demorar.

### Etapa 3 — rolagem e repetição

- *Move rects* da DXGI viram "copie esta área pra lá", sem pixel.
- Cache de pedaços repetidos (ícones, botões) por hash.
