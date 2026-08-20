# The road network

## A road carries one lane in each direction

The player draws one road and gets both directions of travel. A lane is
directed and is the thing a rover drives; the two lanes of a road are
independent, and a rover crosses between them only where changing direction is
legitimate — a turn at a junction, or the end of the road. One road on the map
stays one road on the map: the second lane costs the simulation, not the tile.

**A rover cannot pass the rover in front of it.** There is no lane to move into
and no overtaking anywhere in the network, so a slow rover is everyone's
problem and one badly placed building is a queue you can watch form. That is
the game rather than a limitation of it.

## What it was chosen over

**One lane, both ways along it.** Cheaper still, and it costs the thing the
game is for. Two rovers entering from opposite ends stop being a queue: either
they drive through each other, which makes traffic a decoration, or meeting
head-on needs reservations and passing places, which is more machinery than the
second lane costs.

**Several lanes each way.** This is what the network the road model is drawn
from does, and it buys overtaking. It costs a segment chain per lane and a
lane-change decision in every route a rover ever plans, and it spends all of
that softening the jams. A network that absorbs a slow rover has less to say
than one that backs up behind it.

**One lane, and the player draws the return leg.** Half the network for the
same map, with the saving charged to the player: a road between two buildings
does nothing until a second one is drawn, a dead-end spur strands whatever
drives down it, and the road tool has to ask for a direction before anyone can
build at all. Roads are the first thing a player draws and the wrong place to
put that.

## What it leaves

Where two roads meet, a rover turning across the oncoming lane has to be let
through, and who goes next is the junction's decision. At the end of a road the
two lanes join, so a spur to a single building is drivable: a rover arrives,
turns, and comes back the way the road already goes.
