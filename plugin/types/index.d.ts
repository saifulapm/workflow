declare module 'claude-code' {
  interface PluginState {
    workflow: { interactive: boolean; busy: boolean; asked: boolean; delivered: string[] }
  }
}
