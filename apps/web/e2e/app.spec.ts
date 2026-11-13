import { expect, test, type Page } from '@playwright/test'

const EMAIL = 'admin@example.com'
const PASSWORD = 'un-mot-de-passe-solide'

const DOUBLE = `apiVersion: scaling-waddle.dev/v1
kind: Workflow
metadata: {name: doubler, project: e2e}
spec:
  trigger: {type: manual}
  steps:
    - id: calc
      type: transform
      with: {value: {double: '\${{ event.payload.n * 2 }}'}}
`

const GATED = `apiVersion: scaling-waddle.dev/v1
kind: Workflow
metadata: {name: gated, project: e2e}
spec:
  trigger: {type: manual}
  steps:
    - {id: ask, type: approval, with: {message: 'Valider le déploiement ?'}}
    - {id: go, type: transform, needs: [ask], with: {value: true}}
`

async function signIn(page: Page) {
  await page.goto('/')
  await page.getByLabel('E-mail').fill(EMAIL)
  await page.getByLabel('Mot de passe').fill(PASSWORD)
  await page.getByRole('button', { name: 'Se connecter' }).click()
  await expect(page.getByRole('link', { name: 'Workflows' })).toBeVisible()
}

async function importAndActivate(page: Page, yaml: string, name: string) {
  await page.getByRole('link', { name: 'Workflows' }).click()
  await page.getByLabel('Définition (YAML)').fill(yaml)
  await page.getByRole('button', { name: 'Importer' }).click()
  await expect(page.getByRole('heading', { name })).toBeVisible()
  await page.getByRole('button', { name: 'Activer v1' }).click()
  await expect(page.getByText('v1 · active')).toBeVisible()
}

test.describe.configure({ mode: 'serial' })

test('le premier visiteur crée le compte admin puis exécute un workflow', async ({ page }) => {
  await page.goto('/')
  await expect(page).toHaveURL(/\/login$/)
  await expect(page.getByText('Créez le compte administrateur.')).toBeVisible()
  await page.getByLabel('E-mail').fill(EMAIL)
  await page.getByLabel('Mot de passe').fill(PASSWORD)
  await page.getByRole('button', { name: "Créer l'administrateur" }).click()
  await expect(page.getByRole('link', { name: 'Workflows' })).toBeVisible()

  await importAndActivate(page, DOUBLE, 'doubler')
  await page.getByLabel('Payload JSON').fill('{"n": 21}')
  await page.getByRole('button', { name: 'Lancer' }).click()

  await expect(page).toHaveURL(/\/runs\/.+/)
  await expect(page.getByText('succeeded').first()).toBeVisible({ timeout: 20_000 })
  await expect(page.getByText('"double": 42')).toBeVisible()
})

test("une approbation suspend le run jusqu'à la décision", async ({ page }) => {
  await signIn(page)
  await importAndActivate(page, GATED, 'gated')
  await page.getByRole('button', { name: 'Lancer' }).click()
  await expect(page.getByText('waiting').first()).toBeVisible({ timeout: 20_000 })

  await page.getByRole('link', { name: 'Approbations' }).click()
  await expect(page.getByText('Valider le déploiement ?')).toBeVisible()
  await page.getByRole('button', { name: 'Approuver' }).click()
  await expect(page.getByText('Aucune demande.')).toBeVisible()

  await page.getByRole('link', { name: 'Runs' }).click()
  await page.getByRole('button', { name: /gated|e2e|[0-9a-f]{8}-/ }).first().click()
  await expect(page.getByText('succeeded').first()).toBeVisible({ timeout: 20_000 })
})

test('un administrateur crée un utilisateur qui peut se connecter', async ({ page, browser }) => {
  await signIn(page)
  await page.getByRole('link', { name: 'Réglages' }).click()
  await page.getByLabel('E-mail', { exact: true }).fill('lecteur@example.com')
  await page.getByLabel('Mot de passe', { exact: true }).fill('mot-de-passe-du-lecteur')
  await page.getByRole('button', { name: 'Créer l’utilisateur' }).click()
  await expect(page.getByText('lecteur@example.com')).toBeVisible()

  const other = await browser.newPage()
  await other.goto('/')
  await other.getByLabel('E-mail').fill('lecteur@example.com')
  await other.getByLabel('Mot de passe').fill('mot-de-passe-du-lecteur')
  await other.getByRole('button', { name: 'Se connecter' }).click()
  await expect(other.getByRole('link', { name: 'Workflows' })).toBeVisible()
  await other.getByRole('link', { name: 'Réglages' }).click()
  await expect(other.getByText('Réservé aux administrateurs.')).toBeVisible()
})
