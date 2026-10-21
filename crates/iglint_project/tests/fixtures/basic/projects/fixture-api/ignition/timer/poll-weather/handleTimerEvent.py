def handleTimerEvent():
	result = system.net.httpClient().get("https://example.invalid/api/weather")
	system.tag.writeBlocking(["[default]Plant/Weather"], [result.text])