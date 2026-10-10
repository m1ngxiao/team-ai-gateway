"""Read standard text-token prices from OpenAI's published pricing HTML.

The official page embeds every row (including collapsed rows) in its Astro
``TextTokenPricingTables`` props. Selecting ``tier == standard`` avoids silently
using Batch, Flex, Fast, or Ultrafast rates. No model names or prices are seeded
here. An unknown row format is excluded instead of guessing column meanings.

This module does not download anything. The caller must obtain the HTML from
PRICING_URL over HTTPS and retain its last good result on download/parser errors.
The referenced official component contains the complete long-context rate map
and the context threshold labels. Supply that JS as text; it is never executed.
"""

from __future__ import annotations

from decimal import Decimal, InvalidOperation
from html.parser import HTMLParser
import json
import re
from typing import Any
from urllib.parse import urljoin, urlsplit


PRICING_URL = "https://developers.openai.com/api/docs/pricing"
_SLUG = re.compile(r"[a-z0-9][a-z0-9._-]*\Z")
_SHORT_CONTEXT = re.compile(r" \(<[0-9]+K context length\)\Z")


class PricingFormatError(ValueError):
    """The downloaded page no longer has an unambiguous standard price table."""


def _astro_value(value: Any) -> Any:
    """Decode only Astro's primitive/object and array serialization tags."""
    if not isinstance(value, list) or not value or type(value[0]) is not int:
        raise PricingFormatError("Invalid Astro property encoding")
    if value[0] == 0 and len(value) in (1, 2):
        payload = value[1] if len(value) == 2 else None
        if isinstance(payload, dict):
            return {key: _astro_value(item) for key, item in payload.items()}
        if isinstance(payload, (str, int, Decimal, bool)) or payload is None:
            return payload
    if value[0] == 1 and len(value) == 2 and isinstance(value[1], list):
        return [_astro_value(item) for item in value[1]]
    raise PricingFormatError("Unsupported Astro property encoding")


class _PricingPage(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.standard_rows: list[list[Any]] = []
        self.standard_tables = 0
        self.component_urls: set[str] = set()

    def handle_starttag(self, tag: str, attributes: list[tuple[str, str | None]]) -> None:
        attrs = dict(attributes)
        if tag != "astro-island" or attrs.get("component-export") != "TextTokenPricingTables":
            return
        component_url = attrs.get("component-url")
        if component_url:
            self.component_urls.add(component_url)
        try:
            props = json.loads(attrs.get("props") or "", parse_float=Decimal)
            if _astro_value(props.get("tier")) != "standard":
                return
            rows = _astro_value(props.get("rows"))
        except (json.JSONDecodeError, TypeError, AttributeError) as error:
            raise PricingFormatError("Invalid text-token pricing props") from error
        if not isinstance(rows, list):
            raise PricingFormatError("Standard text-token prices are not rows")
        self.standard_tables += 1
        self.standard_rows.extend(rows)


def pricing_component_url(html: str) -> str:
    """Return the single official JS asset referenced by text pricing tables."""
    page = _PricingPage()
    page.feed(html)
    page.close()
    if len(page.component_urls) != 1:
        raise PricingFormatError("Expected one official text pricing component URL")
    url = urljoin(PRICING_URL, next(iter(page.component_urls)))
    parsed = urlsplit(url)
    if parsed.scheme != "https" or parsed.netloc != "developers.openai.com" or not re.fullmatch(r"/_astro/pricing\.[A-Za-z0-9_-]+\.js", parsed.path):
        raise PricingFormatError("Pricing component URL is outside the official asset path")
    return url


class _LiteralReader:
    """A tiny non-executing parser for the official component's price literals."""
    number = re.compile(r"-?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?")
    identifier = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")

    def __init__(self, source: str, position: int) -> None:
        self.source = source
        self.position = position

    def _space(self) -> None:
        while self.position < len(self.source) and self.source[self.position].isspace():
            self.position += 1

    def value(self, depth: int = 0) -> Any:
        if depth > 8:
            raise PricingFormatError("Price literal is too deeply nested")
        self._space()
        if self.position >= len(self.source):
            raise PricingFormatError("Truncated price literal")
        character = self.source[self.position]
        if character == "{":
            self.position += 1
            result = {}
            self._space()
            while self.position < len(self.source) and self.source[self.position] != "}":
                if self.source[self.position] in ('"', "'", "`"):
                    key = self.value(depth + 1)
                else:
                    match = self.identifier.match(self.source, self.position)
                    if not match:
                        raise PricingFormatError("Unknown price literal key")
                    key = match.group()
                    self.position = match.end()
                self._space()
                if self.position >= len(self.source) or self.source[self.position] != ":":
                    raise PricingFormatError("Missing price literal separator")
                self.position += 1
                if key in result:
                    raise PricingFormatError("Duplicate price literal key")
                result[key] = self.value(depth + 1)
                self._space()
                if self.position < len(self.source) and self.source[self.position] == ",":
                    self.position += 1
                    self._space()
                elif self.position >= len(self.source) or self.source[self.position] != "}":
                    raise PricingFormatError("Invalid price literal object")
            if self.position >= len(self.source):
                raise PricingFormatError("Truncated price literal object")
            self.position += 1
            return result
        if character in ('"', "'", "`"):
            self.position += 1
            end = self.source.find(character, self.position)
            if end < 0:
                raise PricingFormatError("Truncated price literal string")
            value = self.source[self.position:end]
            if "\\" in value or "${" in value or "\n" in value:
                raise PricingFormatError("Price strings must be simple literals")
            self.position = end + 1
            return value
        match = self.number.match(self.source, self.position)
        if not match:
            raise PricingFormatError("Price data contains an unsupported expression")
        self.position = match.end()
        return Decimal(match.group())


def _long_context_prices(component_js: str) -> tuple[dict[str, Any], int | None]:
    candidates = []
    for match in re.finditer(r"\bstandard\s*:\s*(\{)", component_js):
        reader = _LiteralReader(component_js, match.start(1))
        value = reader.value()
        if isinstance(value, dict) and value and all(
            isinstance(slug, str) and _SLUG.fullmatch(slug)
            and isinstance(rates, dict) and set(rates) == {"input", "cachedInput", "cacheWrite", "output"}
            for slug, rates in value.items()
        ):
            candidates.append(value)
    if len(candidates) != 1:
        raise PricingFormatError("Expected one complete standard long-context price map")
    thresholds = []
    for key, comparison in [("shortContextTokens", "≤"), ("longContextTokens", ">")]:
        pattern = rf"\b{key}\s*:\s*\{{\s*label\s*:\s*([`\"']){comparison}([0-9]+(?:\.[0-9]+)?)([KM]?) input tokens\1\s*\}}"
        matches = list(re.finditer(pattern, component_js))
        if len(matches) != 1:
            return candidates[0], None
        amount = Decimal(matches[0].group(2)) * {"": 1, "K": 1000, "M": 1_000_000}[matches[0].group(3)]
        if amount <= 0 or amount != amount.to_integral_value():
            return candidates[0], None
        thresholds.append(int(amount))
    return candidates[0], thresholds[0] + 1 if thresholds[0] == thresholds[1] else None


def _microusd(value: Any, *, optional: bool = False) -> int | None:
    if optional and value in (None, "-"):
        return None
    # Reject booleans, strings, negative/NaN values and sub-micro-dollar prices.
    # Decimal keeps values such as 0.125 and 0.005 exact.
    if type(value) not in (int, Decimal):
        raise PricingFormatError("Price is not a published numeric USD rate")
    try:
        amount = Decimal(value)
        micros = amount * 1_000_000
        if not amount.is_finite() or amount < 0 or micros != micros.to_integral_value():
            raise PricingFormatError("Price cannot be stored exactly in micro USD")
        return int(micros)
    except InvalidOperation as error:
        raise PricingFormatError("Invalid USD rate") from error


def parse_official_prices(html: str, component_js: str | None = None) -> dict[str, dict[str, Any]]:
    """Return exact-slug standard base prices from real official HTML.

    Unknown models and rows with incomplete/changed formats are absent. Raises
    PricingFormatError if the standard table itself is missing or malformed.
    Missing cache-write pricing remains None: this is not a zero-dollar price.
    A caller can use the relay's existing input-price fallback where applicable.
    Supply the referenced JS to verify the full short/long pricing scheme. In
    its absence, potentially tiered rows are excluded. Unknown long thresholds
    exclude those affected models rather than inventing a boundary.
    """
    page = _PricingPage()
    page.feed(html)
    page.close()
    if page.standard_tables != 1 or not page.standard_rows:
        raise PricingFormatError("Expected exactly one standard text-token pricing table")
    long_prices, threshold = _long_context_prices(component_js) if component_js is not None else ({}, None)

    prices: dict[str, dict[str, Any]] = {}
    conflicts: set[str] = set()
    for row in page.standard_rows:
        if not isinstance(row, list) or len(row) not in (4, 5) or not isinstance(row[0], str):
            continue
        # Only the official explicit short-context qualifier can be removed.
        # Long-context qualifiers, annotations, and model aliases stay unmatched.
        slug = _SHORT_CONTEXT.sub("", row[0])
        if not _SLUG.fullmatch(slug):
            continue
        try:
            input_rate = _microusd(row[1])
            cached_rate = _microusd(row[2], optional=True)
            write_rate = _microusd(row[3], optional=True) if len(row) == 5 else None
            output_rate = _microusd(row[-1])
        except PricingFormatError:
            continue
        price = {
            "currency": "USD",
            "price_status": "official",
            "price_source": PRICING_URL,
            "input_microusd_per_1m": input_rate,
            "cached_input_microusd_per_1m": cached_rate,
            "cache_write_microusd_per_1m": write_rate,
            "output_microusd_per_1m": output_rate,
        }
        rate_fields = ["input_microusd_per_1m", "cached_input_microusd_per_1m", "cache_write_microusd_per_1m", "output_microusd_per_1m"]
        tiers = [{"min_input_tokens": 0, **{key: price[key] for key in rate_fields}}]
        if component_js is None and (len(row) == 5 or row[0] != slug):
            continue
        if slug in long_prices:
            rates = long_prices[slug]
            # All-dash long rates explicitly mean there is no priced long tier.
            if any(value != "-" for value in rates.values()):
                if threshold is None:
                    continue
                try:
                    tiers.append({
                        "min_input_tokens": threshold,
                        "input_microusd_per_1m": _microusd(rates["input"]),
                        "cached_input_microusd_per_1m": _microusd(rates["cachedInput"], optional=True),
                        "cache_write_microusd_per_1m": _microusd(rates["cacheWrite"], optional=True),
                        "output_microusd_per_1m": _microusd(rates["output"]),
                    })
                except PricingFormatError:
                    continue
        elif row[0] != slug:
            # Explicitly limited short rows require an explicit long map entry.
            continue
        price["price_tiers"] = tiers
        if slug in prices and prices[slug] != price:
            conflicts.add(slug)
        prices[slug] = price
    for slug in conflicts:
        prices.pop(slug, None)
    return prices


def lookup_official_price(html: str, slug: str, component_js: str | None = None) -> dict[str, Any] | None:
    """Return the published price for an exact model slug, or None if unknown."""
    return parse_official_prices(html, component_js).get(slug)
