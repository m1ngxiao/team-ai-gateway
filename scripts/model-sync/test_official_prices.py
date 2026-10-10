"""Focused parser tests: choose standard rates and fail closed on ambiguities."""

from html import escape
import json
import unittest

from official_prices import PricingFormatError, lookup_official_price, parse_official_prices, pricing_component_url


def table(tier, rows):
    props = {"tier": [0, tier], "rows": [1, [[1, [[0, value] for value in row]] for row in rows]]}
    return '<astro-island component-export="TextTokenPricingTables" props="' + escape(json.dumps(props), quote=True) + '"></astro-island>'


def component(slug="gpt-new-sol", input_rate=4, cached_rate=0.4, write_rate=5, output_rate=15, threshold="272K"):
    rates = json.dumps({slug: {"input": input_rate, "cachedInput": cached_rate, "cacheWrite": write_rate, "output": output_rate}})
    return 'var prices={standard:' + rates + '};var labels={shortContextTokens:{label:`≤' + threshold + ' input tokens`},longContextTokens:{label:`>' + threshold + ' input tokens`}};'


class OfficialPricesTests(unittest.TestCase):
    def test_select_standard_including_collapsed_new_rows(self):
        html = table("batch", [["gpt-new-sol", 1, 0.1, 1.25, 5]]) + table("standard", [["gpt-new-sol", 2, 0.2, 2.5, 10]]) + table("fast", [["gpt-new-sol", 4, 0.4, 5, 20]])
        price = lookup_official_price(html, "gpt-new-sol", component())
        self.assertEqual(price["input_microusd_per_1m"], 2_000_000)
        self.assertEqual(price["cached_input_microusd_per_1m"], 200_000)
        self.assertEqual(price["cache_write_microusd_per_1m"], 2_500_000)
        self.assertEqual(price["output_microusd_per_1m"], 10_000_000)
        self.assertIsNone(lookup_official_price(html, "gpt-new-sol-alias", component()))
        self.assertEqual(price["price_tiers"][1]["min_input_tokens"], 272001)
        self.assertEqual(price["price_tiers"][1]["output_microusd_per_1m"], 15_000_000)

    def test_missing_cache_write_is_none(self):
        price = lookup_official_price(table("standard", [["gpt-old", 1, 0.125, 10]]), "gpt-old")
        self.assertIsNone(price["cache_write_microusd_per_1m"])
        self.assertEqual(price["cached_input_microusd_per_1m"], 125_000)

    def test_context_qualifiers_do_not_replace_short_rate(self):
        html = table("standard", [["gpt-old (<272K context length)", 2, 0.2, "-", 10], ["gpt-old (>272K context length)", 4, 0.4, "-", 15]])
        self.assertEqual(lookup_official_price(html, "gpt-old", component(slug="gpt-old", write_rate="-"))["input_microusd_per_1m"], 2_000_000)

    def test_long_threshold_missing_or_ambiguous_returns_none(self):
        html = table("standard", [["gpt-new-sol", 2, 0.2, 2.5, 10]])
        self.assertIsNone(lookup_official_price(html, "gpt-new-sol"))
        self.assertIsNone(lookup_official_price(html, "gpt-new-sol", component().replace("longContextTokens", "unknownLabel")))
        self.assertIsNone(lookup_official_price(html, "gpt-new-sol", component().replace(">272K", ">400K")))

    def test_component_has_no_expression_execution(self):
        html = table("standard", [["gpt-new-sol", 2, 0.2, 2.5, 10]])
        with self.assertRaises(PricingFormatError):
            lookup_official_price(html, "gpt-new-sol", component().replace('"input": 4', '"input": process.env.KEY'))

    def test_component_url_is_official_same_host(self):
        for url in ["https://evil.example/_astro/pricing.test.js", "https://developers.openai.com.evil.example/_astro/pricing.test.js", "http://developers.openai.com/_astro/pricing.test.js", "https://developers.openai.com:443/_astro/pricing.test.js", "/unrelated.js"]:
            html = table("standard", [["gpt-old", 1, 0.1, 10]]).replace('component-export=', 'component-url="' + url + '" component-export=')
            with self.assertRaises(PricingFormatError):
                pricing_component_url(html)
        html = table("standard", [["gpt-old", 1, 0.1, 10]]).replace('component-export=', 'component-url="/_astro/pricing.test.js?dpl=release" component-export=')
        self.assertEqual(pricing_component_url(html), "https://developers.openai.com/_astro/pricing.test.js?dpl=release")

    def test_conflicts_and_unknown_shape_are_excluded(self):
        html = table("standard", [["gpt-good", 2, 0.2, 10], ["gpt-conflict", 2, 0.2, 10], ["gpt-conflict", 3, 0.3, 15], ["gpt-shape", 2, 0.2, 2.5, 10, 999], ["gpt-negative", -2, 0.2, 10], ["gpt-text", "$2.00", 0.2, 10]])
        self.assertEqual(set(parse_official_prices(html)), {"gpt-good"})

    def test_missing_or_duplicate_standard_table_fails_closed(self):
        for html in ["<html>temporary error</html>", table("batch", [["gpt-new", 1, 0.1, 5]]), table("standard", [["gpt-new", 1, 0.1, 5]]) * 2]:
            with self.assertRaises(PricingFormatError):
                parse_official_prices(html)

    def test_literal_decimal_micro_usd_precision(self):
        html = table("standard", [["gpt-future", 0.1, 0.005, 0.125, 0.5]])
        js = component(slug="gpt-future", input_rate=0.2, cached_rate=0.01, write_rate=0.25, output_rate=0.75)
        price = lookup_official_price(html, "gpt-future", js)
        self.assertEqual(price["cached_input_microusd_per_1m"], 5000)
        self.assertEqual(price["cache_write_microusd_per_1m"], 125000)
        self.assertEqual(price["price_tiers"][1]["output_microusd_per_1m"], 750000)


if __name__ == "__main__":
    unittest.main()
