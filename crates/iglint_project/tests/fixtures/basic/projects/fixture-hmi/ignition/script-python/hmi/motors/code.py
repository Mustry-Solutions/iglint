import core.tags

log = system.util.getLogger("fixture.hmi.motors")


def startMotor(tagPath):
	log.info("start %s" % tagPath)
	system.tag.writeBlocking([tagPath + "/Cmd/Start"], [True])


def isRunning(tagPath):
	return bool(core.tags.readOne(tagPath + "/Running", False))


class MotorRow(object):
	def __init__(self, name, speed):
		self.name = name
		self.speed = speed

	def asDict(self):
		return {"name": self.name, "speed": self.speed}


def listMotors(line):
	folder = core.tags.lineFolder(line)
	results = system.tag.browse(folder, {"tagType": "UdtInstance"}).getResults()
	return [{"tagPath": str(r["fullPath"]), "title": r["name"], "selected": False} for r in results]