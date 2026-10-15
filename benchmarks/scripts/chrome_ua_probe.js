(async () => {
  await new Promise((done) => setTimeout(done, WAIT_MS));
  const data = navigator.userAgentData;
  const named = (brands) => brands ? brands.map((b) => b.brand + "/" + b.version) : null;
  let worker = null;
  try {
    const source = "postMessage({ua: navigator.userAgent, brands: navigator.userAgentData ? navigator.userAgentData.brands.map((b) => b.brand + '/' + b.version) : null})";
    const w = new Worker(URL.createObjectURL(new Blob([source])));
    worker = await new Promise((done) => { w.onmessage = (e) => done(e.data); setTimeout(() => done(null), 3000); });
  } catch (e) {
    worker = { error: String(e) };
  }
  let high = null;
  try {
    high = data ? await data.getHighEntropyValues(["fullVersionList", "platformVersion", "architecture"]) : null;
  } catch (e) {
    high = null;
  }
  return {
    ua: navigator.userAgent,
    webdriver: navigator.webdriver,
    brands: named(data && data.brands),
    platform: data ? data.platform : null,
    full_version_list: named(high && high.fullVersionList),
    platform_version: high ? high.platformVersion : null,
    architecture: high ? high.architecture : null,
    worker,
    api_calls: performance.getEntriesByType("resource").map((e) => e.name).filter((name) => name.includes("/api/")).slice(0, 50),
    body: document.body ? document.body.innerText.slice(0, 20000) : ""
  };
})()
