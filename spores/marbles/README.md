# Marbles

A records-backed issue tracker for fleets of coding agents.

The first binary Spore package. It exports the CLI surface and imports
`soil:records`, `soil:unibus`, and the WASI CLI/runtime interfaces.

```sh
./scripts/spore-pack.sh lock spores/marbles
./scripts/spore-pack.sh pack spores/marbles -o dist/marbles.spore
```
