const LOOPBACK_HOSTS = ['localhost', '127.0.0.1', '[::1]']

/**
 * Preview address as the browser has to request it.
 *
 * A new channel is created with a preview on `127.0.0.1`. That only works in
 * a browser on the server itself; from another computer it points at the
 * visitor's own machine. Such an address is served by the host that delivered
 * the panel instead. Addresses of other hosts are left unchanged.
 */
export function resolvePreviewUrl(previewUrl: string, page: { protocol: string; host: string; hostname: string }) {
    let url: URL

    try {
        url = new URL(previewUrl)
    } catch {
        return previewUrl
    }

    if (!LOOPBACK_HOSTS.includes(url.hostname) || LOOPBACK_HOSTS.includes(page.hostname)) {
        return previewUrl
    }

    // Assigning `url.host` would keep the old port when the page has none.
    return `${page.protocol}//${page.host}${url.pathname}${url.search}${url.hash}`
}
