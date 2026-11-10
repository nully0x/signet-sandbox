// Shared status vocabulary: one mapping from wire statuses to badge
// tones, used by the rail, the detail header, and the node cards.

export type Tone = 'neutral' | 'accent' | 'live' | 'busy' | 'dead'

export const TONE_COLOR: Record<Tone, string> = {
  neutral: 'var(--dim)',
  accent: 'var(--accent)',
  live: 'var(--green)',
  busy: 'var(--amber)',
  dead: 'var(--red)',
}

const STATUS_TONES: Record<string, Tone> = {
  ready: 'live',
  stopped: 'neutral',
  provisioning: 'busy',
  starting: 'busy',
}

export function statusTone(status: string): Tone {
  return STATUS_TONES[status] ?? 'dead'
}
