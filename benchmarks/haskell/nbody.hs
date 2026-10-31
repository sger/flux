data Body = Body !Double !Double !Double !Double !Double !Double !Double

solarMass, daysPerYear, dt :: Double
solarMass = 4.0 * 3.141592653589793 * 3.141592653589793
daysPerYear = 365.24
dt = 0.01

planet :: Double -> Double -> Double -> Double -> Double -> Double -> Double -> Body
planet x y z vx vy vz m
  = Body x y z (vx * daysPerYear) (vy * daysPerYear) (vz * daysPerYear) (m * solarMass)

initial :: [Body]
initial =
  [ Body 0 0 0 0 0 0 solarMass
  , planet 4.84143144246472090e+00 (-1.16032004402742839e+00) (-1.03622044471123109e-01)
           1.66007664274403694e-03 7.69901118419740425e-03 (-6.90460016972063023e-05)
           9.54791938424326609e-04
  , planet 8.34336671824457987e+00 4.12479856412430479e+00 (-4.03523417114321381e-01)
           (-2.76742510726862411e-03) 4.99852801234917238e-03 2.30417297573763929e-05
           2.85885980666130812e-04
  , planet 1.28943695621391310e+01 (-1.51111514016986312e+01) (-2.23307578892655734e-01)
           2.96460137564761618e-03 2.37847173959480950e-03 (-2.96589568540237556e-05)
           4.36624404335156298e-05
  , planet 1.53796971148509165e+01 (-2.59193146099879641e+01) 1.79258772950371181e-01
           2.68067772490389322e-03 1.62824170038242295e-03 (-9.51592254519715870e-05)
           5.15138902046611451e-05
  ]

offsetMomentum :: [Body] -> [Body]
offsetMomentum bodies
  = case bodies of
      Body x y z _ _ _ m : planets ->
        Body x y z (negate px / solarMass) (negate py / solarMass) (negate pz / solarMass) m : planets
      [] -> []
  where
    (px, py, pz) = foldl step (0, 0, 0) bodies
    step (ax, ay, az) (Body _ _ _ vx vy vz m) = (ax + vx * m, ay + vy * m, az + vz * m)

energy :: [Body] -> Double
energy = go 0
  where
    go e [] = e
    go e (b@(Body _ _ _ vx vy vz m) : rest)
      = go (pairs (e + 0.5 * m * (vx * vx + vy * vy + vz * vz)) b rest) rest
    pairs e _ [] = e
    pairs e b@(Body x y z _ _ _ m) (Body ox oy oz _ _ _ om : rest)
      = let dx = x - ox
            dy = y - oy
            dz = z - oz
        in pairs (e - (m * om) / sqrt (dx * dx + dy * dy + dz * dz)) b rest

-- Each body sums the pull of every other body in index order, which rounds
-- the same way as the usual loop over pairs.
kick :: [Body] -> Int -> Body -> Body
kick others i b@(Body _ _ _ vx0 vy0 vz0 _) = go 0 vx0 vy0 vz0 others
  where
    Body x y z _ _ _ m = b
    go _ vx vy vz [] = Body (x + dt * vx) (y + dt * vy) (z + dt * vz) vx vy vz m
    go j vx vy vz (Body ox oy oz _ _ _ om : rest)
      | j == i = go (j + 1) vx vy vz rest
      | otherwise =
          let dx = x - ox
              dy = y - oy
              dz = z - oz
              d2 = dx * dx + dy * dy + dz * dz
              mag = dt / (d2 * sqrt d2)
          in go (j + 1) (vx - dx * om * mag) (vy - dy * om * mag) (vz - dz * om * mag) rest

advance :: [Body] -> [Body]
advance bodies = zipWith (kick bodies) [0 ..] bodies

run :: Int -> [Body] -> [Body]
run 0 bodies = bodies
-- Forcing every body each step keeps the list from growing into a chain of
-- unevaluated steps; the strict fields then force each body completely.
run n bodies = let next = advance bodies in foldr seq () next `seq` run (n - 1) next

main = do
  let bodies = offsetMomentum initial
  print (round (energy bodies * 1.0e9) :: Integer)
  print (round (energy (run 50000 bodies) * 1.0e9) :: Integer)
