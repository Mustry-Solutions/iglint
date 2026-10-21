def readOne(path, default=None):
	"""Read a single tag value, returning default on bad quality."""
	qv = system.tag.readBlocking([path])[0]
	if not qv.quality.isGood():
		return default
	return qv.value


def lineFolder(line):
	return "[default]Plant/%s" % line