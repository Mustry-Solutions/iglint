def handleMessage(payload):
	system.util.getLogger("fixture.api").info("rebuild %r" % payload)
	return {"done": True}