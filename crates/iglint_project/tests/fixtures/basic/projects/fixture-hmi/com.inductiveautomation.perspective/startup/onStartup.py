def onStartup(session):
	session.custom.selectedLine = "Line1"
	system.util.getLogger("fixture.session").info("session %s" % session.props.id)