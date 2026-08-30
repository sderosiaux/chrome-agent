const { JSDOM } = require('jsdom');
const fs = require('node:fs');
const path = require('node:path');
const extract = require('../../vendor/extract.js');

const FIXTURES = path.resolve(__dirname, '..', 'fixtures');

function loadFixture(name) {
  return fs.readFileSync(path.join(FIXTURES, name), 'utf-8');
}

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

module.exports = { extractFromHTML, extractFromHTMLWithSelector, loadFixture, FIXTURES };
