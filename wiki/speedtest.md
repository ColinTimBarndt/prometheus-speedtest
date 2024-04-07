# Speedtest

Speedtests are performed by recording the size of each chunk as $b_x$ and the time it took in seconds as $t_x$ for each data point $1\le x\le N$. Additionally, we define the rate $\delta_x \coloneqq \frac{b_x}{t_x}$.

We also define the total bytes sent/received and total time.

$$
B \coloneqq \sum_{x=1}^N b_x
\qquad
T \coloneqq \sum_{x=1}^N t_x
\qquad
\Delta \coloneqq \frac{B}{T}
$$

Data is then aggregated using the [weighted arithmetic mean] $\mu$ and its variance $\sigma^2$, with $t_x$ being the weight, as follows:

$$
\mu \coloneqq \frac{\sum_{x=1}^N t_x \cdot \delta_x}{T}
\qquad
\sigma^2 \coloneqq \frac{\sum_{x=1}^N t_x \cdot (b_i-\mu)^2}{T}
$$

The mean can easily be calculated using the total bytes and seconds:

$$
\mu
= \frac{\sum_{x=1}^N t_x \cdot \delta_x}{T}
= \frac{\sum_{x=1}^N b_x}{T}
= \frac{B}{T}
= \Delta
$$

[weighted arithmetic mean]: https://en.wikipedia.org/wiki/Weighted_arithmetic_mean
