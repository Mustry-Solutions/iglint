"""Formatting helpers shared by every project."""

UNITS = {"rpm": "rev/min", "degC": u"\u00b0C"}


def withUnit(value, unit):
	return "%s %s" % (value, UNITS.get(unit, unit))


def percent(value, digits=1):
	if value is None:
		return "-"
	return ("%%.%df%%%%" % digits) % (value * 100.0)