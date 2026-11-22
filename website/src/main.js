import { createApp } from 'vue'
import '@fontsource-variable/space-grotesk'
import '@fontsource/jetbrains-mono/400.css'
import '@fontsource/jetbrains-mono/700.css'
import '@mdi/font/css/materialdesignicons.css'
import 'vuetify/styles'
import './styles/main.css'
import App from './App.vue'
import vuetify from './plugins/vuetify'
import { applyDocumentLocale, i18n } from './i18n'

const app = createApp(App)

app.use(vuetify)
app.use(i18n)
applyDocumentLocale()
app.mount('#app')