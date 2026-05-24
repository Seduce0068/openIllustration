/* @refresh reload */
import { render } from 'solid-js/web'
import { checkBrowser } from './browser-check'
import './index.css'
import App from './App.tsx'

if (checkBrowser()) {
  const root = document.getElementById('root')
  render(() => <App />, root!)
}
