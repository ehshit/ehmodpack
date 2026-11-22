<script setup>
import { useI18n } from 'vue-i18n'

defineProps({
  page: {
    type: String,
    default: 'home'
  }
})

const { t } = useI18n()
</script>

<template>
  <header class="topbar" style="background: #1c1726 !important; background-color: #1c1726 !important;">
    <div class="topbar-inner">
      <a class="topbar-brand" href="/">{{ t('nav.brand') }}</a>
      <nav class="topnav" :aria-label="t('nav.label')">
        <a href="/" :class="{ active: page === 'home' }">{{ t('nav.home') }}</a>
        <a href="/downloads" :class="{ active: page === 'downloads' }">{{ t('nav.downloads') }}</a>
        <a href="/privacy" :class="{ active: page === 'privacy' }">{{ t('nav.privacy') }}</a>
      </nav>
    </div>
  </header>
</template>