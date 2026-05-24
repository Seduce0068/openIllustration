// src/browser-check.ts
export function checkBrowser() {
    const ua = navigator.userAgent;
    const isChromium = ua.includes("Chrome") || ua.includes("Edg");
    const isFirefox = ua.includes("Firefox");

    if (!isChromium && !isFirefox) {
        document.body.innerHTML = \`
            <div class="flex items-center justify-center min-h-screen bg-black text-white p-4">
                <div class="border border-red-500 p-8 max-w-lg text-center">
                    <h1 class="text-xl font-bold mb-4">Unsupported Browser</h1>
                    <p>This guild requires a Chromium-based (Chrome, Brave, Edge) or Firefox browser for security and JPKI compliance.</p>
                </div>
            </div>
        \`;
        return false;
    }
    return true;
}
