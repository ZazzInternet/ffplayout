import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    WEB_STEP_ACTIONS,
    WEB_STEP_FIELDS,
    isWebSource,
    newWebPage,
    webPageId,
    webPlaylistItem,
    webSource,
} from '../webPages.ts'

test('web sources resolve to page ids', () => {
    assert.equal(webSource(12), 'web://12')
    assert.equal(webPageId('web://12'), 12)
    assert.equal(webPageId('WEB://12/'), 12)
    assert.equal(webPageId('web://0'), undefined)
    assert.equal(webPageId('web://board'), undefined)
    assert.equal(webPageId('https://example.org'), undefined)
    assert.equal(isWebSource('/media/web/clip.mp4'), false)
    assert.equal(isWebSource(undefined), false)
})

test('every step action defines its fields', () => {
    assert.deepEqual([...WEB_STEP_ACTIONS].sort(), Object.keys(WEB_STEP_FIELDS).sort())
})

test('a web page becomes a playlist entry without a source timeline', () => {
    const page = { ...newWebPage(), id: 4, name: 'Dashboard' }

    assert.deepEqual(webPlaylistItem(page, 90, 'uid-1'), {
        uid: 'uid-1',
        begin: 0,
        title: 'Dashboard',
        source: 'web://4',
        in: 0,
        out: 90,
        duration: 90,
        ad: false,
    })
})
