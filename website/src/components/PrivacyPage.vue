<script setup>
import { useI18n } from 'vue-i18n'

const { t } = useI18n()

const lastUpdated = `Last Updated: ${__PRIVACY_UPDATED_AT__}`
</script>

<template>
  <v-main class="site-main privacy-page">
    <section class="privacy-text">
      <div class="section-wrap">
        <h2>{{ t('privacyPage.title') }}</h2>
        <h2>{{ lastUpdated }}</h2>
        <p>{{ t('privacyPage.intro') }}</p>

        <h2>{{ t('privacyPage.websiteTitle') }}</h2>
        <h3>{{ t('privacyPage.websiteHostingTitle') }}</h3>
        <p>{{ t('privacyPage.websiteHostingText') }}</p>
        <h3>{{ t('privacyPage.websiteDataTitle') }}</h3>
        <p>{{ t('privacyPage.websiteDataText') }}</p>

        <h2>{{ t('privacyPage.appTitle') }}</h2>
        <h3>{{ t('privacyPage.appLoginTitle') }}</h3>
        <p>{{ t('privacyPage.appLoginText') }}</p>
        <h3>{{ t('privacyPage.appModrinthTitle') }}</h3>
        <p>{{ t('privacyPage.appModrinthText') }}</p>
      </div>
    </section>
  </v-main>
</template>
