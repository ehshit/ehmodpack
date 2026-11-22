<script setup>
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
</script>

<template>
  <footer id="site-footer" class="bottom-bar">
    <div class="bottom-inner">
      <div class="bottom-note">
        <span>{{ t('footer.made') }}</span>
      </div>

      <nav class="bottom-nav" :aria-label="t('footer.label')">
        <a href="/">{{ t('footer.home') }}</a>
        <a href="/downloads">{{ t('footer.downloads') }}</a>
        <a href="/privacy">{{ t('footer.privacyLink') }}</a>
      </nav>
    </div>
  </footer>
</template>