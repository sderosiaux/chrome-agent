const { JSDOM } = require('jsdom');
const extract = require('../../vendor/extract.js');

function extractFromHTML(html, limit = 20) {
  const dom = new JSDOM(html);
  const result = extract(dom.window.document, limit);
  return JSON.parse(result);
}

function extractFromHTMLWithSelector(html, selector, limit = 20) {
  const dom = new JSDOM(html);
  const scope = dom.window.document.querySelector(selector);
  if (!scope) return { items: [], hint: `Selector ${selector} not found` };
  const result = extract(scope, limit);
  return JSON.parse(result);
}

module.exports = { extractFromHTML, extractFromHTMLWithSelector };
