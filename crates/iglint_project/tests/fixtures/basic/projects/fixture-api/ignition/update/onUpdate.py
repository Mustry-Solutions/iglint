def onUpdate(actor, resources):
	system.util.getLogger("fixture.api").info("%s changed %d resources" % (actor, len(resources["modified"])))