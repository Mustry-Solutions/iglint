def handleTimerEvent():
	plant.pumps.refresh("North")
	plant.pumps.refresh("South", True)
