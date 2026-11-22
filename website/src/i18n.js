import { createI18n } from 'vue-i18n'

import enUS from './locales/en-US/ui.json'

const localeModules = import.meta.glob('./locales/*/ui.json', { eager: true, import: 'default' })
const messages = {}

for (const [path, value] of Object.entries(localeModules)) {
  const match = path.match(/\.\/locales\/([^/]+)\/ui\.json$/)
  if (match) messages[match[1]] = value
}

messages['en-US'] = enUS

const STORAGE_KEY = 'ehmodpack.locale'
const clean = (value) => value.trim().replace('_', '-')

function storedLocale() {
  try {
    return localStorage.getItem(STORAGE_KEY)
  } catch {
    return null
  }
}

function queryLocale() {
  if (typeof window === 'undefined') return null
  return new URLSearchParams(window.location.search).get('lang')
}

function matchLocale(raw) {
  if (!raw) return null
  const tag = clean(raw)
  if (messages[tag]) return tag
  const base = tag.split('-')[0]
  const found = Object.keys(messages).find((id) => id.split('-')[0].toLowerCase() === base.toLowerCase())
  return found || null
}

function preferredLocale() {
  const candidates = [queryLocale(), storedLocale(), ...(navigator.languages || [])]
  for (const candidate of candidates) {
    const found = matchLocale(candidate)
    if (found) return found
  }
  return 'en-US'
}

export const availableLocales = Object.keys(messages).sort()

export const localeNames = {
  'en-US': 'English (United States)',
  'de-DE': 'German',
  'es-419': 'Spanish (Latin America)',
  'es-ES': 'Spanish',
  'fr-FR': 'French',
  'pt-BR': 'Portuguese (Brazil)',
  'ja-JP': 'Japanese',
  'zh-CN': 'Chinese (Simplified)',
  'ru-RU': 'Russian'
}

export const i18n = createI18n({
  legacy: false,
  locale: preferredLocale(),
  fallbackLocale: 'en-US',
  messages
})

export function setLocale(value) {
  const found = matchLocale(value)
  if (!found) return false
  i18n.global.locale.value = found
  document.documentElement.lang = found
  try {
    localStorage.setItem(STORAGE_KEY, found)
  } catch {
    void 0
  }
  applyDocumentLocale()
  return true
}

export function applyDocumentLocale() {
  const locale = i18n.global.locale.value
  document.documentElement.lang = locale
  document.title = i18n.global.t('meta.title')
  const description = document.querySelector('meta[name="description"]')
  if (description) description.setAttribute('content', i18n.global.t('meta.description'))
}

export function localeLabel(code) {
  return localeNames[code] || code
}