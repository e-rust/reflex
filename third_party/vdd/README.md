# Virtual Display Driver (monitor virtual)

Driver Indirect Display (IddCx, modo usuário) que cria um monitor que não
existe. O Reflex liga esse monitor quando a máquina fica sem monitor ativo
(servidor sem tela, VM Hyper-V com o console fechado) — ver
`docs/levantamento.md` §4.2.

- Projeto: https://github.com/VirtualDrivers/Virtual-Display-Driver (MIT, `LICENSE`)
- Versão: release `25.7.23`, arquivo `VirtualDisplayDriver-x86.Driver.Only.zip`
  (sha256 `e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a`; apesar
  do nome, é o pacote x64 — `NTamd64` no `.inf`)
- Assinatura: `mttvdd.cat` e `MttVDD.dll` assinados pela SignPath Foundation
- Hardware ID: `Root\MttVDD`

Os arquivos ficam **byte a byte** como no release (o `.cat` assina o `.inf`);
o `.gitattributes` os marca como binários. São embutidos no `reflex.exe`
(`app/src/vdd.rs`) e extraídos em `%ProgramData%\Reflex\vdd` na instalação.

Para atualizar: baixar o novo `*.Driver.Only.zip`, conferir a assinatura
(`Get-AuthenticodeSignature`), substituir os três arquivos e esta nota.
