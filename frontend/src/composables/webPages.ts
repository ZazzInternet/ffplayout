export const WEB_SOURCE_SCHEME = 'web://'

export const WEB_STEP_ACTIONS: WebStepAction[] = [
    'wait_for',
    'click',
    'type',
    'press',
    'wait',
    'goto',
    'script',
    'fullscreen',
]

/** Which inputs a step shows; the backend validates the same requirements. */
export const WEB_STEP_FIELDS: Record<WebStepAction, { selector: boolean; value: boolean; seconds: boolean }> = {
    wait: { selector: false, value: false, seconds: true },
    wait_for: { selector: true, value: false, seconds: true },
    click: { selector: true, value: false, seconds: true },
    type: { selector: true, value: true, seconds: true },
    press: { selector: false, value: true, seconds: false },
    goto: { selector: false, value: true, seconds: false },
    script: { selector: false, value: true, seconds: false },
    fullscreen: { selector: true, value: false, seconds: false },
}

export function isWebSource(source: string | undefined | null): boolean {
    return (source ?? '').toLowerCase().startsWith(WEB_SOURCE_SCHEME)
}

/** Returns the page id of a `web://<id>` playlist source. */
export function webPageId(source: string | undefined | null): number | undefined {
    if (!isWebSource(source)) return undefined

    const id = (source ?? '').slice(WEB_SOURCE_SCHEME.length).replace(/\/+$/, '')

    return /^[1-9]\d*$/.test(id) ? Number(id) : undefined
}

export function webSource(id: number): string {
    return `${WEB_SOURCE_SCHEME}${id}`
}

export function newWebStep(action: WebStepAction = 'wait_for'): WebStep {
    return { action, selector: '', value: '', seconds: 0, optional: false, secret: false }
}

export function newWebPage(): WebPage {
    return { id: 0, channel_id: 0, name: '', url: 'https://', steps: [], audio: true, zoom: 1, preroll: 20 }
}

/** A playlist entry that shows the page for `duration` seconds. */
export function webPlaylistItem(page: WebPage, duration: number, uid: string): PlaylistItem {
    return {
        uid,
        begin: 0,
        title: page.name,
        source: webSource(page.id),
        in: 0,
        out: duration,
        duration,
        ad: false,
    }
}
