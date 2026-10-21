def onTagChange(initialChange, newValue, previousValue, event, executionCount):
	if initialChange:
		return
	path = str(event.tagPath)
	system.util.getLogger("fixture.api").info("%s -> %s" % (path, newValue.value))