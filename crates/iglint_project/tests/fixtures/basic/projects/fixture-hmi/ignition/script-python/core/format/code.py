def withUnit(value, unit):
	return "%s %s" % (value, unit)


def percent(value, digits=0):
	return ("%%.%df%%%%" % digits) % ((value or 0) * 100.0)