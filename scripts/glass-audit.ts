// Cross-checks the glass flags of every vehicle in `vehicles.meta` with the
// collision glass materials in its `.yft`. Read-only.
//
//   bun scripts/glass-audit.ts <resources dir> [--out audit.json]
//
// Needs `yft_glass` built:  cargo build --release -p cw-core --example yft_glass
// Flag numbers and names: https://docs.fivem.net/docs/game-references/vehicle-references/vehicle-flags/
// Material names: materials.dat of the game (docs/materiales in astra-dev).

import { Glob } from 'bun'
import { spawnSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const GLASS = {
  120: 'CAR_GLASS_WEAK',
  121: 'CAR_GLASS_MEDIUM',
  122: 'CAR_GLASS_STRONG',
  123: 'CAR_GLASS_BULLETPROOF',
  124: 'CAR_GLASS_OPAQUE',
} as const
const FLAG_UNBREAKABLE = 'FLAG_HAS_BULLETPROOF_GLASS'
const FLAG_RESISTANT = 'FLAG_HAS_BULLET_RESISTANT_GLASS'

interface Material {
  type: number
  flags: number
  polygons: number
}
interface GlassJson {
  file: string
  error?: string
  materials?: Material[]
  windows?: number | null
}
interface Row {
  model: string
  meta: string
  yft: string | null
  flags: string[]
  glass: Record<string, number>
  windows: number | null
  verdict: string
  note: string
}

const args = process.argv.slice(2)
const outIdx = args.indexOf('--out')
const out = outIdx >= 0 ? args[outIdx + 1] : null
const root = resolve(args.find((a, i) => !a.startsWith('--') && i !== outIdx + 1) ?? '')
if (!args.length) {
  console.error('usage: bun scripts/glass-audit.ts <resources dir> [--out audit.json]')
  process.exit(2)
}
const exe = resolve(
  import.meta.dir,
  '../target/release/examples/yft_glass' + (process.platform === 'win32' ? '.exe' : ''),
)

// vehicles.meta: model name and flags per item
const metas: { model: string; flags: string[]; meta: string }[] = []
for (const f of new Glob('**/vehicles.meta').scanSync({ cwd: root, absolute: true })) {
  const text = readFileSync(f, 'utf8')
  for (const m of text.matchAll(
    /<modelName>([^<]+)<\/modelName>[\s\S]*?<flags>([^<]*)<\/flags>/g,
  )) {
    metas.push({
      model: m[1].trim().toLowerCase(),
      flags: m[2].match(/FLAG_[A-Za-z0-9_]+/g) ?? [],
      meta: f,
    })
  }
}

// yft per model name (not the _hi variants)
const yfts = new Map<string, string[]>()
for (const f of new Glob('**/*.yft').scanSync({ cwd: root, absolute: true })) {
  const base = f.split(/[\\/]/).pop()!.toLowerCase().replace(/\.yft$/, '')
  if (base.endsWith('_hi')) continue
  const list = yfts.get(base) ?? []
  list.push(f)
  yfts.set(base, list)
}

// run yft_glass in batches
const wanted = [...new Set(metas.flatMap((m) => yfts.get(m.model) ?? []))]
const glassByFile = new Map<string, GlassJson>()
for (let i = 0; i < wanted.length; i += 60) {
  const batch = wanted.slice(i, i + 60)
  const r = spawnSync(exe, ['--json', ...batch], { encoding: 'utf8', maxBuffer: 1 << 28 })
  for (const line of r.stdout.split('\n')) {
    if (!line.trim()) continue
    const j = JSON.parse(line) as GlassJson
    glassByFile.set(j.file.replace(/\\/g, '/'), j)
  }
}

function judge(flags: string[], glass: Record<string, number>, yft: string | null, err?: string) {
  if (!yft) return ['sin-yft', 'no hay .yft suelto con ese nombre (puede ser add-on o streaming de otro resource)']
  if (err) return ['yft-ilegible', err]
  const types = Object.keys(glass).map(Number)
  const unbreakableFlag = flags.includes(FLAG_UNBREAKABLE)
  const resistantFlag = flags.includes(FLAG_RESISTANT)
  const has123 = types.includes(123)
  const hasAny = types.length > 0
  if (!hasAny) return ['sin-cristal', 'ningún polígono de cristal (120 a 124) en la colisión']
  if (unbreakableFlag && !has123)
    return ['flag-irrompible-sin-material', 'tiene FLAG_HAS_BULLETPROOF_GLASS pero ningún polígono CAR_GLASS_BULLETPROOF']
  if (has123 && !unbreakableFlag && !resistantFlag)
    return ['material-irrompible-sin-flag', 'hay CAR_GLASS_BULLETPROOF pero ningún flag de cristal']
  if (has123 && resistantFlag && !unbreakableFlag)
    return ['material-irrompible-con-flag-resistente', 'CAR_GLASS_BULLETPROOF con FLAG_HAS_BULLET_RESISTANT_GLASS']
  if (resistantFlag && !has123 && types.every((t) => t === 120))
    return ['resistente-con-cristal-debil', 'flag resistente pero todo el cristal es CAR_GLASS_WEAK']
  if (unbreakableFlag && has123) return ['irrompible-consistente', 'flag y material irrompibles']
  if (resistantFlag) return ['resistente-consistente', 'flag resistente con cristal normal']
  return ['normal', 'sin flag de cristal, cristal normal']
}

const rows: Row[] = []
for (const m of metas) {
  const files = yfts.get(m.model) ?? []
  const yft = files[0] ?? null
  const g = yft ? glassByFile.get(yft.replace(/\\/g, '/')) : undefined
  const glass: Record<string, number> = {}
  for (const mat of g?.materials ?? [])
    if (mat.type in GLASS) glass[mat.type] = (glass[mat.type] ?? 0) + mat.polygons
  const [verdict, note] = judge(m.flags, glass, yft, g?.error)
  rows.push({
    model: m.model,
    meta: m.meta.replace(root, '').replace(/\\/g, '/'),
    yft: yft ? yft.replace(root, '').replace(/\\/g, '/') : null,
    flags: m.flags.filter((f) => f === FLAG_UNBREAKABLE || f === FLAG_RESISTANT),
    glass,
    windows: g?.windows ?? null,
    verdict,
    note: files.length > 1 ? `${note} (${files.length} .yft con ese nombre)` : note,
  })
}

const counts = new Map<string, number>()
for (const r of rows) counts.set(r.verdict, (counts.get(r.verdict) ?? 0) + 1)
console.log(`models in vehicles.meta: ${rows.length}`)
for (const [v, n] of [...counts].toSorted((a, b) => b[1] - a[1])) console.log(String(n).padStart(5), v)
if (out) writeFileSync(out, JSON.stringify({ root, exe: join(exe), counts: Object.fromEntries(counts), rows }, null, 2))
