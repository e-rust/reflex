//! `reflex.exe`. Subsistema de console de propósito: a linha de comando
//! (`--status`, `--install`…) escreve nele; aberto pelo Explorer, o manifesto
//! (`consoleAllocationPolicy`) e o `modes` evitam a janela de console.

fn main() {
    reflex::modes::main();
}
