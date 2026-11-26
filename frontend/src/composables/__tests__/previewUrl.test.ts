import assert from 'node:assert/strict'
import { test } from 'node:test'
import { resolvePreviewUrl } from '../previewUrl.ts'

const lan = { protocol: 'http:', host: '192.168.1.20:8787', hostname: '192.168.1.20' }
const proxy = { protocol: 'https:', host: 'tv.example.org', hostname: 'tv.example.org' }
const local = { protocol: 'http:', host: '127.0.0.1:8787', hostname: '127.0.0.1' }
const initial = 'http://127.0.0.1:8787/public/1/live/stream.m3u8'

test('the initial loopback preview follows the host that serves the panel', () => {
    assert.equal(resolvePreviewUrl(initial, lan), 'http://192.168.1.20:8787/public/1/live/stream.m3u8')
    assert.equal(resolvePreviewUrl(initial, proxy), 'https://tv.example.org/public/1/live/stream.m3u8')
    assert.equal(
        resolvePreviewUrl('http://localhost:8787/live/a.m3u8?x=1', lan),
        'http://192.168.1.20:8787/live/a.m3u8?x=1',
    )
})

test('a panel opened on the server keeps the loopback address', () => {
    assert.equal(resolvePreviewUrl(initial, local), initial)
})

test('other hosts, relative and empty addresses are unchanged', () => {
    for (const url of ['https://cdn.example.org/live/stream.m3u8', '/public/1/live/stream.m3u8', '']) {
        assert.equal(resolvePreviewUrl(url, lan), url)
    }
})
