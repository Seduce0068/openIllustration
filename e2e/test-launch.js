const { chromium } = require('playwright');

(async () => {
  const browser = await chromium.launch({
    executablePath: '/usr/bin/google-chrome'
  });
  console.log('Browser launched successfully');
  await browser.close();
})();
