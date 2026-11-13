# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: app.spec.ts >> le premier visiteur crée le compte admin puis exécute un workflow
- Location: e2e/app.spec.ts:46:1

# Error details

```
Error: expect(locator).toBeVisible() failed

Locator: getByText('succeeded').first()
Expected: visible
Timeout: 20000ms
Error: element(s) not found

Call log:
  - Expect "toBeVisible" getByText('succeeded').first() with timeout 20000ms
  - waiting for getByText('succeeded').first()

```

```yaml
- navigation:
  - link "Overview":
    - /url: /
  - link "Workflows":
    - /url: /workflows
  - link "Runs":
    - /url: /runs
  - link "Approbations":
    - /url: /approvals
  - link "Intégrations":
    - /url: /integrations
  - link "Audit":
    - /url: /audit
  - link "Réglages":
    - /url: /settings
- main:
  - heading "Run" [level=1]
  - text: cbc6047a-eee0-42d2-9572-8a2d55868fc6 queued
  - button "Annuler"
  - heading "Événement" [level=2]
  - paragraph: manual manual.run
  - text: "{ \"n\": 21 }"
  - heading "Étapes" [level=2]
  - list:
    - listitem: calc pending
```

# Test source

```ts
  1  | import { expect, test, type Page } from '@playwright/test'
  2  | 
  3  | const EMAIL = 'admin@example.com'
  4  | const PASSWORD = 'un-mot-de-passe-solide'
  5  | 
  6  | const DOUBLE = `apiVersion: scaling-waddle.dev/v1
  7  | kind: Workflow
  8  | metadata: {name: doubler, project: e2e}
  9  | spec:
  10 |   trigger: {type: manual}
  11 |   steps:
  12 |     - id: calc
  13 |       type: transform
  14 |       with: {value: {double: '\${{ event.payload.n * 2 }}'}}
  15 | `
  16 | 
  17 | const GATED = `apiVersion: scaling-waddle.dev/v1
  18 | kind: Workflow
  19 | metadata: {name: gated, project: e2e}
  20 | spec:
  21 |   trigger: {type: manual}
  22 |   steps:
  23 |     - {id: ask, type: approval, with: {message: 'Valider le déploiement ?'}}
  24 |     - {id: go, type: transform, needs: [ask], with: {value: true}}
  25 | `
  26 | 
  27 | async function signIn(page: Page) {
  28 |   await page.goto('/')
  29 |   await page.getByLabel('E-mail').fill(EMAIL)
  30 |   await page.getByLabel('Mot de passe').fill(PASSWORD)
  31 |   await page.getByRole('button', { name: 'Se connecter' }).click()
  32 |   await expect(page.getByRole('link', { name: 'Workflows' })).toBeVisible()
  33 | }
  34 | 
  35 | async function importAndActivate(page: Page, yaml: string, name: string) {
  36 |   await page.getByRole('link', { name: 'Workflows' }).click()
  37 |   await page.getByLabel('Définition (YAML)').fill(yaml)
  38 |   await page.getByRole('button', { name: 'Importer' }).click()
  39 |   await expect(page.getByRole('heading', { name })).toBeVisible()
  40 |   await page.getByRole('button', { name: 'Activer v1' }).click()
  41 |   await expect(page.getByText('v1 · active')).toBeVisible()
  42 | }
  43 | 
  44 | test.describe.configure({ mode: 'serial' })
  45 | 
  46 | test('le premier visiteur crée le compte admin puis exécute un workflow', async ({ page }) => {
  47 |   await page.goto('/')
  48 |   await expect(page).toHaveURL(/\/login$/)
  49 |   await expect(page.getByText('Créez le compte administrateur.')).toBeVisible()
  50 |   await page.getByLabel('E-mail').fill(EMAIL)
  51 |   await page.getByLabel('Mot de passe').fill(PASSWORD)
  52 |   await page.getByRole('button', { name: "Créer l'administrateur" }).click()
  53 |   await expect(page.getByRole('link', { name: 'Workflows' })).toBeVisible()
  54 | 
  55 |   await importAndActivate(page, DOUBLE, 'doubler')
  56 |   await page.getByLabel('Payload JSON').fill('{"n": 21}')
  57 |   await page.getByRole('button', { name: 'Lancer' }).click()
  58 | 
  59 |   await expect(page).toHaveURL(/\/runs\/.+/)
> 60 |   await expect(page.getByText('succeeded').first()).toBeVisible({ timeout: 20_000 })
     |                                                     ^ Error: expect(locator).toBeVisible() failed
  61 |   await expect(page.getByText('"double": 42')).toBeVisible()
  62 | })
  63 | 
  64 | test("une approbation suspend le run jusqu'à la décision", async ({ page }) => {
  65 |   await signIn(page)
  66 |   await importAndActivate(page, GATED, 'gated')
  67 |   await page.getByRole('button', { name: 'Lancer' }).click()
  68 |   await expect(page.getByText('waiting').first()).toBeVisible({ timeout: 20_000 })
  69 | 
  70 |   await page.getByRole('link', { name: 'Approbations' }).click()
  71 |   await expect(page.getByText('Valider le déploiement ?')).toBeVisible()
  72 |   await page.getByRole('button', { name: 'Approuver' }).click()
  73 |   await expect(page.getByText('Aucune demande.')).toBeVisible()
  74 | 
  75 |   await page.getByRole('link', { name: 'Runs' }).click()
  76 |   await page.getByRole('button', { name: /gated|e2e|[0-9a-f]{8}-/ }).first().click()
  77 |   await expect(page.getByText('succeeded').first()).toBeVisible({ timeout: 20_000 })
  78 | })
  79 | 
  80 | test('un administrateur crée un utilisateur qui peut se connecter', async ({ page, browser }) => {
  81 |   await signIn(page)
  82 |   await page.getByRole('link', { name: 'Réglages' }).click()
  83 |   await page.getByLabel('E-mail', { exact: true }).fill('lecteur@example.com')
  84 |   await page.getByLabel('Mot de passe', { exact: true }).fill('mot-de-passe-du-lecteur')
  85 |   await page.getByRole('button', { name: 'Créer l’utilisateur' }).click()
  86 |   await expect(page.getByText('lecteur@example.com')).toBeVisible()
  87 | 
  88 |   const other = await browser.newPage()
  89 |   await other.goto('/')
  90 |   await other.getByLabel('E-mail').fill('lecteur@example.com')
  91 |   await other.getByLabel('Mot de passe').fill('mot-de-passe-du-lecteur')
  92 |   await other.getByRole('button', { name: 'Se connecter' }).click()
  93 |   await expect(other.getByRole('link', { name: 'Workflows' })).toBeVisible()
  94 |   await other.getByRole('link', { name: 'Réglages' }).click()
  95 |   await expect(other.getByText('Réservé aux administrateurs.')).toBeVisible()
  96 | })
  97 | 
```