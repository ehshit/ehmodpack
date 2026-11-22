<script setup>
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const { t } = useI18n()

const features = computed(() => [
  {
    title: t('features.resourceTitle'),
    text: t('features.resourceText'),
    icon: 'mdi-layers-outline',
    tape: '/tapes/order.gif'
  },
  {
    title: t('features.importTitle'),
    text: t('features.importText'),
    icon: 'mdi-file-import-outline',
    tape: '/tapes/new-from-mrpack.gif'
  },
  {
    title: t('features.verifyTitle'),
    text: t('features.verifyText'),
    icon: 'mdi-shield-check-outline',
    tape: '/tapes/verify.gif'
  },
  {
    title: t('features.buildTitle'),
    text: t('features.buildText'),
    icon: 'mdi-package-variant-closed',
    tape: '/tapes/build.gif'
  }
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
</script>

<template>
  <v-main class="site-main">
    <section id="home" class="hero">
      <div class="section-wrap hero-grid">
        <div class="hero-copy">
          <h1>{{ t('hero.headline') }}</h1>
          <p class="hero-description">{{ t('hero.description') }}</p>
          <div class="hero-actions">
            <v-btn class="flat-button primary" href="/downloads">{{ t('hero.download') }}</v-btn>
            <v-btn class="flat-button" href="https://docs.ehis.gay/stuff/ehmodpack/" target="_blank" rel="noreferrer">{{ t('hero.docs') }}</v-btn>
          </div>
        </div>

        <div class="hero-media">
          <img src="/tapes/readme.gif" :alt="t('hero.gifAlt')" />
        </div>
      </div>
    </section>

    <section class="features">
      <div class="section-wrap">
        <h2>{{ t('features.title') }}</h2>
        <div class="feature-list">
          <article v-for="feature in features" :key="feature.title" class="feature-row">
            <div class="feature-copy">
              <v-icon class="feature-icon" :icon="feature.icon" size="64"></v-icon>
              <h3>{{ feature.title }}</h3>
              <p>
                <template v-for="(part, index) in splitInlineCode(feature.text)" :key="index">
                  <code v-if="part.code">{{ part.text }}</code>
                  <template v-else>{{ part.text }}</template>
                </template>
              </p>
            </div>
            <div class="feature-media">
              <img :src="feature.tape" :alt="feature.title" />
            </div>
          </article>
        </div>
      </div>
    </section>

    <section class="publish-note">
      <div class="section-wrap">
        <p>{{ t('publish.note') }}</p>
      </div>
    </section>
  </v-main>
</template>