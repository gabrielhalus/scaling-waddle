# scaling-waddle

Plateforme d'automatisation événementielle, open source et self-hosted : `Trigger → Workflow → Steps → Actions → Audit`.

Spécifications : [docs/specs-v1.md](docs/specs-v1.md).

## Développement

Prérequis : Rust stable, Node 24, pnpm, [dekit](https://dekit.run).

```sh
pnpm install
dekit up            # api (8080), worker, web (Vite)
```

Contrôles :

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm --filter web lint && pnpm --filter web typecheck && pnpm --filter web test
```

## Configuration

| Variable | Défaut | Rôle |
| --- | --- | --- |
| `SW_JWT_SECRET` | requis, 32 caractères min | Signature des JWT |
| `SW_DATABASE_PATH` | `data/scaling-waddle.db` | Fichier SQLite |
| `SW_BIND` | `127.0.0.1:8080` | Adresse d'écoute de l'API |
| `SW_LOG_FORMAT` | texte | `json` pour des logs structurés |

## Licence

Apache-2.0
