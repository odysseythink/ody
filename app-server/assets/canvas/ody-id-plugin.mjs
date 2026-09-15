// ody canvas id 注入插件（切片 2，契约见 project-canvas-proposal §12/§12.1）。
// 由 ody-app-server 生成到 <root>/node_modules/.ody/ 下；放在 node_modules
// 内是为了不改用户工程文件，同时让本文件能解析到工程自己的 @babel/core。
// 行为：dev transform 阶段对 .tsx/.jsx 跑一遍 babel，给 JSX 元素注入
// 确定性 data-ody-id（sha1(file:line:component:tag) 前 10 位），并把
// id → 源码位置 全量快照写到 <root>/.ody/canvas-id-map.json，供 odyBox
// 点选解析。任何失败一律静默降级（不注入、不写 manifest），绝不让
// dev server 起不来。
import crypto from 'node:crypto'
import path from 'node:path'
import { mkdirSync, realpathSync, writeFileSync } from 'node:fs'

const SOURCE_RE = /\.(tsx|jsx)$/

// babel 插件本体（与 odyBox spikes/ody-id-inject 预研同逻辑）
function makeOdyIdBabelPlugin({ types: t }) {
  return {
    name: 'ody-id',
    visitor: {
      JSXOpeningElement(elPath, state) {
        const node = elPath.node
        if (typeof node.name.name !== 'string') return // <Foo.Bar/> 等成员表达式：P0 跳过
        const has = node.attributes.some((a) => a.type === 'JSXAttribute' && a.name.name === 'data-ody-id')
        if (has) return
        const line = node.loc?.start.line
        if (line == null) return

        let component = 'anonymous'
        let p = elPath
        while ((p = p.parentPath)) {
          if (p.isFunctionDeclaration() && p.node.id?.type === 'Identifier') {
            component = p.node.id.name
            break
          }
          if ((p.isArrowFunctionExpression() || p.isFunctionExpression()) && p.parentPath) {
            const parent = p.parentPath
            if (parent.isVariableDeclarator() && parent.node.id.type === 'Identifier') {
              component = parent.node.id.name
              break
            }
            if (parent.isCallExpression()) continue // HOC 包裹，继续向上
          }
        }

        const file = state.filename ? path.relative(state.cwd ?? process.cwd(), state.filename) : 'unknown'
        const tag = node.name.name
        const key = `${file}:${line}:${component}:${tag}`
        const id = 'o' + crypto.createHash('sha1').update(key).digest('hex').slice(0, 10)

        node.attributes.push(t.jsxAttribute(t.jsxIdentifier('data-ody-id'), t.stringLiteral(id)))
        const manifest = state.opts?.manifest
        if (Array.isArray(manifest)) manifest.push({ id, file, line, component, tag })
      },
    },
  }
}

export default function odyCanvasIdPlugin({ root } = {}) {
  // realpath 对齐：macOS /tmp 等符号链接会让 babel 的 filename 变成真实路径，
  // 而 root 未解析，path.relative 会产出 ../../private/tmp/... 这种脏路径。
  const projectRoot = realpathSync(root ?? process.cwd())
  const entries = new Map()
  const manifestFile = path.join(projectRoot, '.ody', 'canvas-id-map.json')
  let writeTimer = null

  const flush = () => {
    if (writeTimer) return
    writeTimer = setTimeout(() => {
      writeTimer = null
      try {
        mkdirSync(path.dirname(manifestFile), { recursive: true })
        writeFileSync(
          manifestFile,
          JSON.stringify({ version: 1, generatedAtMs: Date.now(), entries: Object.fromEntries(entries) }),
        )
      } catch {
        // manifest 是 best-effort：写失败不影响 dev server
      }
    }, 150)
  }

  return {
    name: 'ody-canvas-id',
    enforce: 'pre',
    async transform(code, id) {
      if (!SOURCE_RE.test(id) || id.includes('/node_modules/')) return null
      let babel = null
      try {
        babel = await import('@babel/core')
      } catch {
        return null // 工程没有 @babel/core（如非 React 的 Vite 工程）：降级不注入
      }
      const manifest = []
      let result = null
      try {
        result = await babel.transformAsync(code, {
          filename: id,
          cwd: projectRoot,
          sourceMaps: true,
          configFile: false,
          babelrc: false,
          parserOpts: { plugins: ['jsx', 'typescript'] },
          plugins: [[makeOdyIdBabelPlugin, { manifest }]],
        })
      } catch {
        return null // 解析失败（语法超集边缘）：跳过本文件，不阻塞 dev
      }
      for (const entry of manifest) {
        entries.set(entry.id, { file: entry.file, line: entry.line, component: entry.component, tag: entry.tag })
      }
      flush()
      if (!result?.code) return null
      return { code: result.code, map: result.map ?? null }
    },
  }
}
