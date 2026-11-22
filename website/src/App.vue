<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import LandingPage from './components/LandingPage.vue'
import DownloadsPage from './components/DownloadsPage.vue'
import PrivacyPage from './components/PrivacyPage.vue'
import SiteHeader from './components/SiteHeader.vue'
import BottomBar from './components/BottomBar.vue'

const route = ref('home')

function readRoute() {
  const path = window.location.pathname.replace(/\/+$/, '')
  route.value = path === '/downloads' ? 'downloads' : path === '/privacy' ? 'privacy' : 'home'
}

onMounted(() => {
  readRoute()
  window.addEventListener('popstate', readRoute)
})

onUnmounted(() => {
  window.removeEventListener('popstate', readRoute)
})

const currentPage = computed(() => {
  if (route.value === 'downloads') return DownloadsPage
  if (route.value === 'privacy') return PrivacyPage
  return LandingPage
})
</script>

<template>
  <v-app class="app-shell">
    <SiteHeader :page="route" />
    <component :is="currentPage" />
    <BottomBar />
  </v-app>
</template>
