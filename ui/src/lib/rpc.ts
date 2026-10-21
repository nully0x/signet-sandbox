// Thin JSON-RPC client for the provisioning API. The bearer token lives
// in localStorage; the dashboard is same-origin, so every request is a
// plain fetch to /api/rpc.

export class RpcError extends Error {
  code: number
  constructor(code: number, message: string) {
    super(message)
    this.code = code
  }
}

const TOKEN_KEY = 'signet.token'

export const UNAUTHENTICATED = -32002

export function storedToken(): string {
  return localStorage.getItem(TOKEN_KEY) ?? ''
}

export function saveToken(token: string) {
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
}

/// A `#token=sgn_...` fragment wins over a stored one: `signet up --ui`
/// can hand the freshly minted profile token straight to the browser.
export function tokenFromFragment(): string | null {
  const hash = location.hash.replace(/^#/, '')
  for (const part of new URLSearchParams(hash)) {
    if (part[0] === 'token') return part[1]
  }
  return null
}

export async function call<T>(method: string, params: unknown = {}): Promise<T> {
  const response = await fetch('/api/rpc', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${storedToken()}`,
    },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
  })
  if (!response.ok) {
    throw new RpcError(-32700, `http ${response.status}`)
  }
  const payload = await response.json()
  if (payload.error) {
    throw new RpcError(payload.error.code, payload.error.message ?? 'rpc error')
  }
  return payload.result as T
}

export interface EnvSummary {
  id: string
  name: string
  status: string
  created_at: string
  expires_at: string | null
}

export interface BundleAuth {
  type: string
  value: string
}

export interface LightningConnection {
  implementation: string
  rest_url: string
  tls_cert: string
  auth: BundleAuth
}

export interface Bundle {
  environment_id: string
  status: string
  rpc_url: string
  rpc_auth: string | null
  indexer_endpoint: string | null
  explorer_url: string | null
  faucet_url: string | null
  lightning: Record<string, LightningConnection>
  signet_challenge: string
  block_policy: string
  versions: Record<string, string> | null
  expires_at: string | null
}

export const api = {
  list: () => call<EnvSummary[]>('environment.list'),
  get: (id: string) => call<Bundle>('environment.get', { id }),
}
