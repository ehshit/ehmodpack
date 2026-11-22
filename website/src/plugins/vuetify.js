import { createVuetify } from 'vuetify'
import { aliases, mdi } from 'vuetify/iconsets/mdi'
import * as components from 'vuetify/components'
import * as directives from 'vuetify/directives'

export default createVuetify({
  components,
  directives,
  icons: {
    defaultSet: 'mdi',
    aliases,
    sets: { mdi }
  },
  theme: {
    defaultTheme: 'ehmodpackDark',
    themes: {
      ehmodpackDark: {
        dark: true,
        colors: {
          primary: '#d8b4fe',
          secondary: '#9ae6d4',
          tertiary: '#f6c177',
          background: '#101018',
          surface: '#191922',
          'surface-variant': '#22222d',
          'on-background': '#f4f1f8',
          'on-surface': '#f4f1f8',
          'on-surface-variant': '#b5aec3',
          outline: '#3b3549',
          error: '#ff8ba7'
        }
      }
    }
  },
  defaults: {
    VBtn: {
      rounded: false,
      variant: 'flat'
    }
  }
})
