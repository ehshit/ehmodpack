<script setup>
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import releaseData from '../data/downloads.json'

const { t } = useI18n()
const copied = ref('')
const activeTab = ref('windows')
const detectedTab = ref('')
const activeCommandTab = ref('winget')
const activeLinuxTab = ref('archives')

function detectTab() {
  if (typeof navigator === 'undefined') return ''

  const ua = navigator.userAgent

  if (/android/i.test(ua)) return ''
  if (/iphone|ipad|ipod/i.test(ua)) return 'macos'
  if (/mac/i.test(ua)) return 'macos'
  if (/win/i.test(ua)) return 'windows'
  if (/linux|x11|cros/i.test(ua)) return 'linux'
  return ''
}

onMounted(() => {
  detectedTab.value = detectTab()
  if (detectedTab.value) activeTab.value = detectedTab.value
})

const wingetCommand = computed(() => `winget install --id ${releaseData.winget.package} --version ${releaseData.winget.version} -e`)
const cargoCommand = computed(() => t('downloads.cargoCommand'))

function methodIcon(icon) {
  const icons = {
    terminal: 'mdi-console',
    download: 'mdi-download',
    package: 'mdi-package-variant-closed'
  }
  return icons[icon] || 'mdi-download'
}

function archiveMethod(item) {
  return {
    id: item.arch,
    name: item.system,
    desc: t('downloads.directDescription'),
    icon: 'download',
    href: item.url,
    format: item.file.endsWith('.zip') ? 'zip' : 'tar.gz',
    version: releaseData.version,
    file: item.file,
    cta: t('downloads.get'),
    recommended: item.arch === 'windows-x86_64'
  }
}

function linuxPackageMethod(item, format) {
  const url = format === 'deb' ? item.deb : item.rpm

  return {
    id: `${item.system}-${format}`,
    name: `${item.system} ${format}`,
    desc: t('downloads.packageDescription', { system: item.system }),
    icon: 'package',
    href: url,
    format,
    version: releaseData.version,
    file: url.split('/').pop(),
    cta: t('downloads.get')
  }
}

const linuxGroups = computed(() => ({
  archives: releaseData.archives
    .filter((item) => item.arch.startsWith('linux'))
    .map(archiveMethod),
  deb: releaseData.linuxPackages
    .filter((item) => item.deb)
    .map((item) => linuxPackageMethod(item, 'deb')),
  rpm: releaseData.linuxPackages
    .filter((item) => item.rpm)
    .map((item) => linuxPackageMethod(item, 'rpm'))
}))

const linuxTabs = computed(() => [
  { id: 'archives', label: t('downloads.linuxTabs.archives') },
  { id: 'deb', label: t('downloads.linuxTabs.deb') },
  { id: 'rpm', label: t('downloads.linuxTabs.rpm') }
])

const activeLinuxMethods = computed(() => linuxGroups.value[activeLinuxTab.value] || [])

const commandMethods = computed(() => {
  const methods = {}

  if (activeTab.value === 'windows') {
    methods.winget = {
      id: 'winget',
      name: t('downloads.wingetTitle'),
      desc: t('downloads.wingetDescription'),
      icon: 'terminal',
      command: wingetCommand.value,
      cta: t('downloads.copy'),
      note: t('downloads.wingetNote', { ver: releaseData.winget.version })
    }
  }

  methods.cargo = {
    id: 'cargo',
    name: t('downloads.cargoTitle'),
    desc: t('downloads.cargoDescription'),
    icon: 'terminal',
    command: cargoCommand.value,
    cta: t('downloads.copy')
  }

  return methods
})

const activeCommandMethod = computed(() => commandMethods.value[activeCommandTab.value] || commandMethods.value.cargo)

const platforms = computed(() => {
  const archives = releaseData.archives
  const of = (prefix) => archives.filter((item) => item.arch.startsWith(prefix)).map(archiveMethod)

  return [
    {
      id: 'windows',
      name: t('downloads.tabs.windows'),
      icon: 'mdi-microsoft-windows',
      methods: of('windows')
    },
    {
      id: 'macos',
      name: t('downloads.tabs.macos'),
      icon: 'mdi-apple',
      methods: of('macos')
    },
    {
      id: 'linux',
      name: t('downloads.tabs.linux'),
      icon: 'mdi-penguin',
      methods: linuxGroups.value.archives
    }
  ]
})

const detectedLabel = computed(() => {
  const platform = platforms.value.find((item) => item.id === detectedTab.value)
  return platform ? t('downloads.hint', { os: platform.name }) : ''
})

const macOSNotes = computed(() => [
  t('downloads.macOSNote'),
  t('downloads.macOSQuarantineArm'),
  t('downloads.macOSQuarantineIntel')
])

function splitInlineCode(text) {
  return String(text)
    .split(/(`[^`]+`)/g)
    .filter(Boolean)
    .map((part) => {
      const code = part.startsWith('`') && part.endsWith('`')
      return {
        code,
        text: code ? part.slice(1, -1) : part
      }
    })
}

async function copyCommand(method) {
  try {
    await navigator.clipboard.writeText(method.command)
    copied.value = method.id
    window.setTimeout(() => {
      if (copied.value === method.id) copied.value = ''
    }, 1600)
  } catch {
    copied.value = ''
  }
}
</script>

<template>
  <v-main class="site-main downloads-page">
    <section class="downloads-hero">
      <div class="section-wrap">
        <v-icon class="downloads-hero-icon" icon="mdi-download" size="64"></v-icon>
        <h1>{{ t('downloads.title') }}</h1>
      </div>
    </section>

    <section class="downloads-section">
      <div class="section-wrap">
        <div class="download-card">
          <p v-if="detectedLabel" class="dl-hint">{{ detectedLabel }}</p>

          <div class="dl-tabs-wrap">
            <div class="dl-tabs" role="tablist" :aria-label="t('downloads.tabs.label')">
              <button
                v-for="platform in platforms"
                :key="platform.id"
                class="dl-tab"
                :class="{ active: activeTab === platform.id }"
                type="button"
                role="tab"
                :aria-selected="activeTab === platform.id"
                :aria-controls="`panel-${platform.id}`"
                @click="activeTab = platform.id"
              >
                <v-icon :icon="platform.icon" size="24"></v-icon>
                <span>{{ platform.name }}</span>
              </button>
            </div>
          </div>

          <div class="dl-panels">
            <div
              v-for="platform in platforms"
              :id="`panel-${platform.id}`"
              v-show="activeTab === platform.id"
              :key="platform.id"
              class="dl-panel"
              role="tabpanel"
            >
              <div v-if="platform.id === 'linux'" class="dl-command-tabs" role="tablist" :aria-label="t('downloads.linuxTabs.label')">
                <button
                  v-for="tab in linuxTabs"
                  :key="tab.id"
                  class="dl-command-tab"
                  :class="{ active: activeLinuxTab === tab.id }"
                  type="button"
                  role="tab"
                  :aria-selected="activeLinuxTab === tab.id"
                  @click="activeLinuxTab = tab.id"
                >
                  {{ tab.label }}
                </button>
              </div>

              <article
                v-for="method in platform.id === 'linux' ? activeLinuxMethods : platform.methods"
                :key="method.id"
                class="dl-method"
                :class="{ recommended: method.recommended }"
              >
                <div class="dl-method-main">
                  <div class="dl-method-info">
                    <span class="dl-method-icon">
                      <v-icon :icon="methodIcon(method.icon)" size="22"></v-icon>
                    </span>
                    <div class="dl-method-copy">
                      <div class="dl-method-title">
                        <span v-if="method.recommended" class="dl-badge">{{ t('downloads.recommended') }}</span>
                        <strong>{{ method.name }}</strong>
                      </div>
                      <p class="dl-method-desc">{{ method.desc }}</p>
                      <div class="dl-meta">
                        <span class="dl-chip">{{ method.format }}</span>
                        <span class="dl-chip">{{ method.version }}</span>
                      </div>
                      <code class="dl-file">{{ method.file }}</code>
                    </div>
                  </div>

                  <a
                    class="dl-action"
                    :href="method.href"
                    target="_blank"
                    rel="noreferrer"
                  >
                    {{ method.cta }}
                    <v-icon icon="mdi-download" size="16"></v-icon>
                  </a>
                </div>
              </article>

              <div v-if="platform.id === 'macos'" class="dl-panel-notes">
                <p v-for="(note, index) in macOSNotes" :key="index" class="dl-panel-note">
                  <template v-for="(part, partIndex) in splitInlineCode(note)" :key="partIndex">
                    <code v-if="part.code">{{ part.text }}</code>
                    <template v-else>{{ part.text }}</template>
                  </template>
                </p>
              </div>

              <div class="dl-or">
                <span>{{ t('downloads.or') }}</span>
              </div>

              <div v-if="activeTab === 'windows'" class="dl-command-tabs" role="tablist" :aria-label="t('downloads.commandTabsLabel')">
                <button
                  v-for="method in Object.values(commandMethods)"
                  :key="method.id"
                  class="dl-command-tab"
                  :class="{ active: activeCommandTab === method.id }"
                  type="button"
                  role="tab"
                  :aria-selected="activeCommandTab === method.id"
                  @click="activeCommandTab = method.id"
                >
                  {{ method.name }}
                </button>
              </div>

              <article class="dl-method command-method">
                <div class="dl-method-main">
                  <div class="dl-method-info">
                    <span class="dl-method-icon">
                      <v-icon :icon="methodIcon(activeCommandMethod.icon)" size="22"></v-icon>
                    </span>
                    <div class="dl-method-copy">
                      <div class="dl-method-title">
                        <strong>{{ activeCommandMethod.name }}</strong>
                      </div>
                      <p class="dl-method-desc">
                        {{ activeCommandMethod.desc }}
                        <a
                          v-if="activeCommandMethod.id === 'cargo'"
                          class="dl-inline-link"
                          href="https://www.rust-lang.org/tools/install"
                          target="_blank"
                          rel="noreferrer"
                        >{{ t('downloads.rustDownload') }}</a>
                      </p>
                      <p v-if="activeCommandMethod.note" class="dl-method-note">{{ activeCommandMethod.note }}</p>
                    </div>
                  </div>

                  <button class="dl-action" type="button" @click="copyCommand(activeCommandMethod)">
                    {{ copied === activeCommandMethod.id ? t('downloads.copied') : t('downloads.copyCommand') }}
                    <v-icon :icon="copied === activeCommandMethod.id ? 'mdi-check' : 'mdi-content-copy'" size="16"></v-icon>
                  </button>
                </div>

                <div class="dl-command">
                  <span>$</span>
                  <code>{{ activeCommandMethod.command }}</code>
                </div>
              </article>
            </div>
          </div>
        </div>

      </div>
    </section>
  </v-main>
</template>
