def handleScheduleEvent():
	system.util.sendMessage(system.util.getProjectName(), "rebuild-report", {"when": "nightly"})