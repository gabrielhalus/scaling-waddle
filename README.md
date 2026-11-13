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
| `SW_ALLOWED_INTERNAL_NETWORKS` | vide | CIDR internes autorisés pour les appels HTTP (garde SSRF) |
| `SW_MAX_CONCURRENT_RUNS` | `4` | Runs traités en parallèle par le worker (1 à 256) |
| `SW_RETENTION_DAYS` | `30` | Durée de conservation des runs terminés, `0` pour tout garder |
| `SW_SECRET_KEYS` | vide | Clés AES-256 des secrets : `id=hex64,id2=hex64` |
| `SW_SECRET_ACTIVE_KEY` | vide | Identifiant de la clé qui chiffre les nouveaux secrets |
| `SW_AGENT_COMMAND` | vide | Chemin de la CLI `claude` ; active l'étape `agent.run` (désactivée si vide) |
| `SW_AGENT_OAUTH_TOKEN` | vide | Jeton généré par `claude setup-token`, transmis à la CLI |

Pour générer une clé : `openssl rand -hex 32`. Après avoir changé la clé active, `POST /api/v1/secrets/rotate` (admin) rechiffre les secrets existants.

## Licence

Apache-2.0

## Étape `agent.run` (optionnelle)

L'agent est un exécutant facultatif : sans `SW_AGENT_COMMAND`, l'étape échoue avec un message explicite et le reste de la plateforme n'en dépend pas.

```yaml
- id: triage
  type: agent.run
  with:
    prompt: "Résume cette alerte : ${{ event.payload.message }}"
    timeout_seconds: 120   # 1 à 600
    model: sonnet          # optionnel
```

La sortie est `{result, cost_usd, turns}`. L'agent s'appelle par `claude -p`, **sans aucun outil** (génération de texte uniquement), dans un dossier vide supprimé après l'appel et avec un environnement épuré : seuls `PATH` et le jeton lui sont transmis, jamais les secrets de la plateforme.

Authentification : lancez `claude setup-token` avec votre abonnement et placez le jeton dans `SW_AGENT_OAUTH_TOKEN`. L'usage doit rester celui d'une personne dans les quotas de son abonnement ; vérifiez les conditions d'utilisation en vigueur avant tout autre déploiement.

Attention : un prompt construit depuis un payload externe (webhook, GitLab, Uptime Kuma) peut contenir des instructions hostiles. Sans outil, l'agent ne peut rien exécuter, mais sa réponse reste une donnée non fiable : ne la transmettez pas telle quelle à une action sensible sans approbation.
